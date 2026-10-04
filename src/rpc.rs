//! CLI ↔ daemon protocol: newline-delimited JSON-RPC 2.0 over the Unix socket
//! `~/.tome/tome.sock`. Each request and each response is one line of JSON.
//!
//! Errors carry the tome [`ErrorKind`] in `error.data.kind` (plus an optional
//! `hint` / `details`) so the CLI can map them straight onto its exit codes.

use crate::node::Node;
use crate::output::{CliError, CliResult, ErrorKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, ErrorKind as IoErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

mod ssh;

pub const JSONRPC: &str = "2.0";

/// Method of the notifications a streaming request (`run.start {attach}`,
/// `run.watch`) receives before its response; `params` is the event.
pub const EVENT: &str = "run.event";

/// How long a streaming read waits before giving the caller a turn.
const STREAM_POLL: Duration = Duration::from_millis(200);

/// JSON-RPC error codes.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    /// Application error; see `data.kind`.
    pub const APP: i64 = -32000;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    /// The agent making the call (`{run_id, worker?}`), from `TOME_RUN_ID`
    /// and `TOME_WORKER_ID`. Any call from an agent proves it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Response {
            jsonrpc: JSONRPC.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Response {
            jsonrpc: JSONRPC.into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }

    pub fn from_cli_error(id: Value, err: &CliError) -> Self {
        let code = match err.kind {
            ErrorKind::Invalid | ErrorKind::InvalidWorkflow => codes::INVALID_PARAMS,
            ErrorKind::Internal => codes::INTERNAL,
            _ => codes::APP,
        };
        let mut data = json!({ "kind": err.kind.as_str() });
        if let Some(hint) = &err.hint {
            data["hint"] = json!(hint);
        }
        if let Some(details) = &err.details {
            data["details"] = details.clone();
        }
        Response::err(id, code, err.message.clone(), Some(data))
    }
}

impl RpcError {
    pub fn into_cli_error(self) -> CliError {
        let data = self.data.unwrap_or(Value::Null);
        let kind = match data.get("kind").and_then(Value::as_str) {
            Some(kind) => ErrorKind::parse(kind),
            None if self.code == codes::INVALID_PARAMS => ErrorKind::Invalid,
            None => ErrorKind::Internal,
        };
        let mut err = CliError::new(kind, self.message);
        if let Some(hint) = data.get("hint").and_then(Value::as_str) {
            err = err.with_hint(hint);
        }
        if let Some(details) = data.get("details") {
            err = err.with_details(details.clone());
        }
        err
    }
}

/// A connection to a daemon: this machine's over its socket, or a node's
/// through `ssh <dest> <tome> rpc --stdio`. One connection can carry many
/// requests.
pub struct Client {
    conn: Conn,
    next_id: u64,
    /// The node's `daemon.ping` answer, on a connection to a node.
    pub pong: Option<Value>,
    /// How long that ping took.
    pub rtt: Option<Duration>,
}

enum Conn {
    Unix {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
        /// Kept across timeouts: a read can stop mid-line.
        partial: Vec<u8>,
    },
    Ssh(ssh::Conn),
}

/// What a read got.
enum Got {
    Line(Vec<u8>),
    /// Nothing within the timeout.
    Idle,
    Closed,
}

impl Conn {
    fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        match self {
            Conn::Unix { writer, .. } => writer.write_all(line),
            Conn::Ssh(c) => c.write_line(line),
        }
    }

    fn read(&mut self, timeout: Duration) -> CliResult<Got> {
        match self {
            Conn::Unix {
                reader, partial, ..
            } => {
                reader.get_ref().set_read_timeout(Some(timeout))?;
                match reader.read_until(b'\n', partial) {
                    Ok(0) => Ok(Got::Closed),
                    Ok(_) if partial.ends_with(b"\n") => Ok(Got::Line(std::mem::take(partial))),
                    Ok(_) => Ok(Got::Idle),
                    Err(e)
                        if matches!(
                            e.kind(),
                            IoErrorKind::WouldBlock
                                | IoErrorKind::TimedOut
                                | IoErrorKind::Interrupted
                        ) =>
                    {
                        Ok(Got::Idle)
                    }
                    Err(e) => Err(CliError::internal(format!(
                        "failed to read from the daemon: {e}"
                    ))),
                }
            }
            Conn::Ssh(c) => c.read(timeout),
        }
    }

    /// The error for a connection that closed before its answer came.
    fn closed(&mut self, what: &str) -> CliError {
        match self {
            Conn::Unix { .. } => {
                CliError::internal(format!("the daemon closed the connection {what}"))
            }
            Conn::Ssh(c) => c.closed_error(what),
        }
    }
}

/// How long a plain call waits for its answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

impl Client {
    /// Connect to the daemon: the target node's if this command has one
    /// (`--on`), else this machine's. Never starts the local daemon: if
    /// nothing is listening, this fails with a `daemon_not_running` error
    /// carrying a hint.
    pub fn connect(socket: &Path) -> CliResult<Client> {
        match crate::node::target() {
            Some(node) => Client::to_node(node),
            None => Client::local(socket),
        }
    }

    /// Connect to this machine's daemon.
    pub fn local(socket: &Path) -> CliResult<Client> {
        match UnixStream::connect(socket) {
            Ok(stream) => {
                let writer = stream.try_clone()?;
                Ok(Client {
                    conn: Conn::Unix {
                        reader: BufReader::new(stream),
                        writer,
                        partial: Vec::new(),
                    },
                    next_id: 1,
                    pong: None,
                    rtt: None,
                })
            }
            Err(e) if is_not_running(&e) => Err(CliError::daemon_not_running()),
            Err(e) => Err(CliError::internal(format!(
                "failed to connect to daemon at {}: {e}",
                socket.display()
            ))),
        }
    }

    /// Connect to a node's daemon over SSH. The first request is
    /// `daemon.ping`: a node speaking another protocol is refused before
    /// anything else is sent.
    pub fn to_node(node: &Node) -> CliResult<Client> {
        let mut client = Client {
            conn: Conn::Ssh(ssh::Conn::open(node)?),
            next_id: 1,
            pong: None,
            rtt: None,
        };
        let started = Instant::now();
        let pong = client.call("daemon.ping", json!({ "protocol": crate::node::PROTOCOL }))?;
        client.rtt = Some(started.elapsed());
        crate::node::check_protocol(node, &pong)?;
        client.pong = Some(pong);
        Ok(client)
    }

    pub fn call(&mut self, method: &str, params: Value) -> CliResult<Value> {
        self.send(method, params)?;
        match self.conn.read(CALL_TIMEOUT)? {
            Got::Line(line) => parse_response(&line),
            Got::Idle => Err(CliError::internal(
                "timed out waiting for the daemon's response",
            )),
            Got::Closed => Err(self.conn.closed("without responding")),
        }
    }

    /// Make a streaming request: `on_event` gets each `run.event`
    /// notification until the response arrives. While nothing arrives,
    /// `on_idle` is called every [`STREAM_POLL`]; an error from it ends the
    /// stream.
    pub fn stream(
        &mut self,
        method: &str,
        params: Value,
        mut on_event: impl FnMut(&Value),
        mut on_idle: impl FnMut() -> CliResult<()>,
    ) -> CliResult<Value> {
        self.send(method, params)?;
        loop {
            match self.conn.read(STREAM_POLL)? {
                Got::Closed => return Err(self.conn.closed("mid-run")),
                Got::Line(line) => {
                    let msg: Value = serde_json::from_slice(&line).map_err(|e| {
                        CliError::internal(format!("malformed daemon message: {e}"))
                    })?;
                    if msg["method"] == EVENT {
                        on_event(&msg["params"]);
                    } else {
                        return parse_response(&line);
                    }
                }
                Got::Idle => on_idle()?,
            }
        }
    }

    fn send(&mut self, method: &str, params: Value) -> CliResult<()> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request {
            jsonrpc: JSONRPC.into(),
            id: json!(id),
            method: method.into(),
            params,
            // An agent here is no caller of a node's daemon.
            caller: caller().filter(|_| matches!(self.conn, Conn::Unix { .. })),
        };
        let mut line =
            serde_json::to_string(&req).map_err(|e| CliError::internal(e.to_string()))?;
        line.push('\n');
        if let Err(e) = self.conn.write_line(line.as_bytes()) {
            return Err(match &mut self.conn {
                Conn::Ssh(c) => c.closed_error("before the request was sent"),
                Conn::Unix { .. } => {
                    CliError::internal(format!("failed to send request to daemon: {e}"))
                }
            });
        }
        Ok(())
    }
}

/// Who this process is, if tome started it as an agent.
fn caller() -> Option<Value> {
    let var = |k| {
        std::env::var(k)
            .ok()
            .filter(|v: &String| !v.trim().is_empty())
    };
    let run_id = var("TOME_RUN_ID")?;
    Some(json!({ "run_id": run_id, "worker": var("TOME_WORKER_ID") }))
}

fn parse_response(line: &[u8]) -> CliResult<Value> {
    let resp: Response = serde_json::from_slice(line)
        .map_err(|e| CliError::internal(format!("malformed daemon response: {e}")))?;
    match (resp.result, resp.error) {
        (_, Some(err)) => Err(err.into_cli_error()),
        (Some(result), None) => Ok(result),
        (None, None) => Ok(Value::Null),
    }
}

fn is_not_running(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        IoErrorKind::NotFound | IoErrorKind::ConnectionRefused
    )
}

/// One-shot call helper.
pub fn call(socket: &Path, method: &str, params: Value) -> CliResult<Value> {
    Client::connect(socket)?.call(method, params)
}

/// Whether a connection failed because ssh couldn't sign in.
pub fn is_auth_failure(e: &CliError) -> bool {
    ssh::is_auth_failure(e)
}

/// The error for an ssh to `node` that ended (exit `code`, saying `stderr`)
/// before tome answered.
pub fn classify_ssh(node: &Node, code: Option<i32>, stderr: &str) -> CliError {
    ssh::classify(node, code, stderr, "before tome answered")
}
