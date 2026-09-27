//! CLI ↔ daemon protocol: newline-delimited JSON-RPC 2.0 over the Unix socket
//! `~/.tome/tome.sock`. Each request and each response is one line of JSON.
//!
//! Errors carry the tome [`ErrorKind`] in `error.data.kind` (plus an optional
//! `hint` / `details`) so the CLI can map them straight onto its exit codes.

use crate::output::{CliError, CliResult, ErrorKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, ErrorKind as IoErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

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
        Response { jsonrpc: JSONRPC.into(), id, result: Some(result), error: None }
    }

    pub fn err(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Response {
            jsonrpc: JSONRPC.into(),
            id,
            result: None,
            error: Some(RpcError { code, message: message.into(), data }),
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

/// A connection to the daemon. One connection can carry many requests.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    /// Connect to the daemon. Never starts it: if nothing is listening, this
    /// fails with a `daemon_not_running` error carrying a hint.
    pub fn connect(socket: &Path) -> CliResult<Client> {
        match UnixStream::connect(socket) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(300)))?;
                let writer = stream.try_clone()?;
                Ok(Client { reader: BufReader::new(stream), writer, next_id: 1 })
            }
            Err(e) if is_not_running(&e) => Err(CliError::daemon_not_running()),
            Err(e) => Err(CliError::internal(format!(
                "failed to connect to daemon at {}: {e}",
                socket.display()
            ))),
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> CliResult<Value> {
        self.send(method, params)?;
        let mut buf = String::new();
        let n = self
            .reader
            .read_line(&mut buf)
            .map_err(|e| CliError::internal(format!("failed to read daemon response: {e}")))?;
        if n == 0 {
            return Err(CliError::internal("daemon closed the connection without responding"));
        }
        parse_response(buf.as_bytes())
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
        self.reader.get_ref().set_read_timeout(Some(STREAM_POLL))?;
        // Kept across timeouts: a read can stop mid-line.
        let mut buf = Vec::new();
        loop {
            match self.reader.read_until(b'\n', &mut buf) {
                Ok(0) => return Err(CliError::internal("the daemon closed the connection mid-run")),
                Ok(_) if buf.ends_with(b"\n") => {
                    let line = std::mem::take(&mut buf);
                    let msg: Value = serde_json::from_slice(&line)
                        .map_err(|e| CliError::internal(format!("malformed daemon message: {e}")))?;
                    if msg["method"] == EVENT {
                        on_event(&msg["params"]);
                    } else {
                        return parse_response(&line);
                    }
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), IoErrorKind::WouldBlock | IoErrorKind::TimedOut | IoErrorKind::Interrupted) => {
                    on_idle()?
                }
                Err(e) => return Err(CliError::internal(format!("failed to read from the daemon: {e}"))),
            }
        }
    }

    fn send(&mut self, method: &str, params: Value) -> CliResult<()> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request { jsonrpc: JSONRPC.into(), id: json!(id), method: method.into(), params };
        let mut line = serde_json::to_string(&req).map_err(|e| CliError::internal(e.to_string()))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| CliError::internal(format!("failed to send request to daemon: {e}")))
    }
}

fn parse_response(line: &[u8]) -> CliResult<Value> {
    let resp: Response =
        serde_json::from_slice(line).map_err(|e| CliError::internal(format!("malformed daemon response: {e}")))?;
    match (resp.result, resp.error) {
        (_, Some(err)) => Err(err.into_cli_error()),
        (Some(result), None) => Ok(result),
        (None, None) => Ok(Value::Null),
    }
}

fn is_not_running(e: &std::io::Error) -> bool {
    matches!(e.kind(), IoErrorKind::NotFound | IoErrorKind::ConnectionRefused)
}

/// One-shot call helper.
pub fn call(socket: &Path, method: &str, params: Value) -> CliResult<Value> {
    Client::connect(socket)?.call(method, params)
}
