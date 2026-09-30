//! The tome daemon: one per user per machine, listening on `~/.tome/tome.sock`.
//!
//! `tome daemon run` runs it in the foreground (this is what service units and
//! `tome daemon start` execute). Single-instance is enforced with an exclusive
//! `flock` on `~/.tome/daemon.lock`, so a stale socket left by a crash is
//! detected and replaced safely.

use crate::api;
use crate::engine::{Engine, Sink};
use crate::orchestrator;
use crate::output::CliResult;
use crate::paths;
use crate::recovery;
use crate::rpc::{self, codes, Request, Response};
use crate::store::Store;
use anyhow::{bail, Context};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct Daemon {
    started: Instant,
    started_at_unix: u64,
    socket: PathBuf,
    engine: Arc<Engine>,
    _lock: File,
}

impl Daemon {
    fn status(&self) -> Value {
        json!({
            "running": true,
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": self.started.elapsed().as_secs(),
            "started_at_unix": self.started_at_unix,
            "home": paths::tome_home(),
            "socket": self.socket,
            "database": paths::db_path(),
        })
    }

    /// Dispatch one request. Returns `None` for an unknown method, otherwise
    /// the result plus whether the daemon should shut down after the response
    /// has been written.
    fn handle(&self, req: &Request) -> Option<(CliResult<Value>, bool)> {
        let handled = match req.method.as_str() {
            "daemon.ping" => (Ok(json!({ "pong": true })), false),
            "daemon.status" => (Ok(self.status()), false),
            "daemon.shutdown" => (Ok(json!({ "stopping": true })), true),
            method if Engine::handles(method) => (self.engine.dispatch(method, &req.params), false),
            method if api::handles(method) => (
                self.engine
                    .with_store(|store| api::dispatch(store, method, &req.params)),
                false,
            ),
            _ => return None,
        };
        Some(handled)
    }

    /// Release resources and exit the process.
    fn shutdown(&self) -> ! {
        let _ = fs::remove_file(&self.socket);
        self.engine.close();
        eprintln!("tome daemon: shutting down");
        std::process::exit(0);
    }
}

/// Run the daemon in the foreground until it's told to shut down.
pub fn run_foreground() -> anyhow::Result<()> {
    let home = paths::tome_home();
    fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;

    let lock = acquire_lock()?;
    let mut store = Store::open(&paths::db_path(), &paths::runs_dir())?;
    // Before accepting requests: no running run survives its daemon (queued
    // ones do, and start below).
    for run in recovery::recover(&mut store, &orchestrator::Hooks)
        .context("recovering interrupted runs")?
    {
        eprintln!(
            "tome daemon: run {} ({}) marked failed: {}",
            run.id,
            run.workflow_name,
            recovery::REASON
        );
    }
    let socket = paths::socket_path();
    if socket.exists() {
        // We hold the lock, so whatever left this socket behind is gone.
        fs::remove_file(&socket)
            .with_context(|| format!("removing stale socket {}", socket.display()))?;
    }
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;

    let daemon = Arc::new(Daemon {
        started: Instant::now(),
        started_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        socket: socket.clone(),
        engine: Arc::new(Engine::new(store)),
        _lock: lock,
    });
    eprintln!(
        "tome daemon: listening on {} (pid {})",
        socket.display(),
        std::process::id()
    );
    let engine = Arc::clone(&daemon.engine);
    std::thread::spawn(move || engine.resume_queued());
    let engine = Arc::clone(&daemon.engine);
    std::thread::spawn(move || engine.monitor());
    let engine = Arc::clone(&daemon.engine);
    std::thread::spawn(move || engine.trigger_loop());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let daemon = Arc::clone(&daemon);
                std::thread::spawn(move || serve_connection(&daemon, stream));
            }
            Err(e) => eprintln!("tome daemon: accept failed: {e}"),
        }
    }
    Ok(())
}

fn acquire_lock() -> anyhow::Result<File> {
    let path = paths::lock_path();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: flock on a valid, owned file descriptor.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        bail!(
            "another tome daemon is already running (lock held on {})",
            path.display()
        );
    }
    file.set_len(0)?;
    writeln!(&file, "{}", std::process::id())?;
    Ok(file)
}

fn serve_connection(daemon: &Daemon, stream: UnixStream) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let req = serde_json::from_str::<Request>(&line);
        if let Some(agent) = req
            .as_ref()
            .ok()
            .and_then(|r| r.caller.as_ref())
            .and_then(crate::handshake::caller_of)
        {
            daemon.engine.seen(agent);
        }
        let (resp, shutdown) = match req {
            Ok(req) if streams(&req) => match stream_run(daemon, &req, &mut writer) {
                Some(Ok(value)) => (Response::ok(req.id, value), false),
                Some(Err(err)) => (Response::from_cli_error(req.id, &err), false),
                // The caller went away mid-stream.
                None => return,
            },
            Ok(req) => match daemon.handle(&req) {
                Some((Ok(value), shutdown)) => (Response::ok(req.id, value), shutdown),
                Some((Err(err), shutdown)) => (Response::from_cli_error(req.id, &err), shutdown),
                None => {
                    let msg = format!("unknown method `{}`", req.method);
                    (
                        Response::err(req.id, codes::METHOD_NOT_FOUND, msg, None),
                        false,
                    )
                }
            },
            Err(e) => (
                Response::err(
                    Value::Null,
                    codes::PARSE_ERROR,
                    format!("invalid request: {e}"),
                    None,
                ),
                false,
            ),
        };
        if write_line(&mut writer, &resp).is_err() {
            return;
        }
        if shutdown {
            daemon.shutdown();
        }
    }
}

/// Requests answered with a stream of `run.event` notifications before
/// their response: `run.start {attach: true}` and `run.watch`.
fn streams(req: &Request) -> bool {
    req.method == "run.watch" || (req.method == "run.start" && req.params["attach"] == true)
}

/// Serve a streaming request. `None` if the caller disconnected first.
fn stream_run(daemon: &Daemon, req: &Request, writer: &mut UnixStream) -> Option<CliResult<Value>> {
    let mut sink = SocketSink { out: writer };
    let result = if req.method == "run.watch" {
        match api::req_id(&req.params) {
            Ok(id) => {
                daemon
                    .engine
                    .watch(id, req.params["cancel_on_disconnect"] == true, &mut sink)
            }
            Err(e) => Some(Err(e)),
        }
    } else {
        daemon.engine.start_attached(&req.params, &mut sink)
    };
    result.map(|r| r.map(|run| json!(run)))
}

/// Writes events to a client as JSON-RPC notifications.
struct SocketSink<'a> {
    out: &'a mut UnixStream,
}

impl Sink for SocketSink<'_> {
    fn send(&mut self, event: &Value) -> std::io::Result<()> {
        write_line(
            self.out,
            &json!({ "jsonrpc": rpc::JSONRPC, "method": rpc::EVENT, "params": event }),
        )
    }

    /// The client never writes during a stream, so a readable socket with
    /// nothing to read means it closed its end.
    fn gone(&mut self) -> bool {
        let fd = self.out.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: polling and peeking one valid descriptor we own.
        unsafe {
            if libc::poll(&mut pfd, 1, 0) <= 0 {
                return false;
            }
            if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                return true;
            }
            let mut byte = 0u8;
            let n = libc::recv(
                fd,
                (&mut byte as *mut u8).cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            );
            n == 0
                || (n < 0
                    && !matches!(
                        std::io::Error::last_os_error().kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ))
        }
    }
}

fn write_line(out: &mut UnixStream, value: &impl serde::Serialize) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    line.push('\n');
    out.write_all(line.as_bytes())?;
    out.flush()
}
