//! The tome daemon: one per user per machine, listening on `~/.tome/tome.sock`.
//!
//! `tome daemon run` runs it in the foreground (this is what service units and
//! `tome daemon start` execute). Single-instance is enforced with an exclusive
//! `flock` on `~/.tome/daemon.lock`, so a stale socket left by a crash is
//! detected and replaced safely.

use crate::api;
use crate::engine::Engine;
use crate::output::CliResult;
use crate::paths;
use crate::recovery;
use crate::rpc::{codes, Request, Response};
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
            method if api::handles(method) => {
                (self.engine.with_store(|store| api::dispatch(store, method, &req.params)), false)
            }
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
    // Before accepting requests: no run survives its daemon.
    for run in recovery::recover(&mut store, &recovery::NoopHooks).context("recovering interrupted runs")? {
        eprintln!("tome daemon: run {} ({}) marked failed: {}", run.id, run.workflow_name, recovery::REASON);
    }
    let socket = paths::socket_path();
    if socket.exists() {
        // We hold the lock, so whatever left this socket behind is gone.
        fs::remove_file(&socket).with_context(|| format!("removing stale socket {}", socket.display()))?;
    }
    let listener = UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;

    let daemon = Arc::new(Daemon {
        started: Instant::now(),
        started_at_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        socket: socket.clone(),
        engine: Arc::new(Engine::new(store)),
        _lock: lock,
    });
    eprintln!("tome daemon: listening on {} (pid {})", socket.display(), std::process::id());

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
        bail!("another tome daemon is already running (lock held on {})", path.display());
    }
    file.set_len(0)?;
    writeln!(&file, "{}", std::process::id())?;
    Ok(file)
}

fn serve_connection(daemon: &Daemon, stream: UnixStream) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let (resp, shutdown) = match serde_json::from_str::<Request>(&line) {
            Ok(req) => match daemon.handle(&req) {
                Some((Ok(value), shutdown)) => (Response::ok(req.id, value), shutdown),
                Some((Err(err), shutdown)) => (Response::from_cli_error(req.id, &err), shutdown),
                None => {
                    let msg = format!("unknown method `{}`", req.method);
                    (Response::err(req.id, codes::METHOD_NOT_FOUND, msg, None), false)
                }
            },
            Err(e) => (Response::err(Value::Null, codes::PARSE_ERROR, format!("invalid request: {e}"), None), false),
        };
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            return;
        }
        let _ = writer.flush();
        if shutdown {
            daemon.shutdown();
        }
    }
}
