//! The daemon's plugin processes.
//!
//! Each enabled plugin with hooks runs as one persistent child, started with
//! the daemon, speaking newline-delimited JSON-RPC 2.0 over its stdin and
//! stdout. Tome sends `initialize` first; after that it sends requests
//! (`provide`, `release`, `run.before_start`, and the notification hooks) and
//! the plugin may send `notify` requests and `progress` notifications back.
//!
//! A supervisor thread per plugin starts it, restarts it with backoff when
//! it dies, and gives up after [`MAX_FAILURES`] in a row (the plugin is then
//! `failed`, and the user is notified). Calls have timeouts and fail at once
//! when the plugin is dead, so a bad plugin never holds up tome. Nothing here
//! takes the store lock: callers must not hold it while they call a plugin.

use super::manifest::{Hook, Manifest, PROTOCOL};
use super::store::{self, Installed};
use crate::stop::Stop;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Consecutive failed starts or crashes before a plugin is marked failed.
pub const MAX_FAILURES: u32 = 5;
/// How long `initialize` may take.
pub const INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// A hook's timeout when the manifest sets none.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(10);
/// `provide`'s timeout when the manifest sets none.
pub const PROVIDE_TIMEOUT: Duration = Duration::from_secs(600);
/// How long `release` may take unless the manifest says (tearing a machine
/// down is slower than a notification).
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a stopping plugin gets to answer `daemon.stopping` and exit.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// A plugin that stayed up this long starts over with a clean slate.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// Lines queued for a plugin's stdin before it counts as not reading.
const QUEUE: usize = 256;
/// The least time between two notifications about one plugin's hook.
const NOTIFY_EVERY: Duration = Duration::from_secs(60);

/// Time before the next restart after `failures` in a row. The base can be
/// shortened with `TOME_PLUGIN_BACKOFF_MS` (the tests do).
pub fn backoff(failures: u32) -> Duration {
    let base = std::env::var("TOME_PLUGIN_BACKOFF_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1000);
    let exp = failures.saturating_sub(1).min(5);
    Duration::from_millis(base.saturating_mul(1 << exp))
}

// ---------------------------------------------------------------------------
// Errors

/// Why a call to a plugin didn't produce a result.
#[derive(Debug, Clone)]
pub enum CallError {
    /// The plugin isn't running: not started, restarting, failed.
    NotRunning(String),
    Timeout(Duration),
    /// It died, or stopped reading, with the call outstanding.
    Died(String),
    /// It answered with an error.
    Plugin {
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::NotRunning(why) => write!(f, "the plugin isn't running ({why})"),
            CallError::Timeout(d) => write!(f, "the plugin didn't answer within {}s", d.as_secs()),
            CallError::Died(why) => write!(f, "the plugin went away: {why}"),
            CallError::Plugin { message, .. } => write!(f, "{message}"),
        }
    }
}

// ---------------------------------------------------------------------------
// One connection to a running plugin

enum Event {
    Progress(String),
    Done(Result<Value, CallError>),
}

enum Outgoing {
    Line(String),
    /// Close the plugin's stdin.
    Close,
}

struct Session {
    out: SyncSender<Outgoing>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Sender<Event>>>,
    alive: AtomicBool,
}

impl Session {
    fn new(out: SyncSender<Outgoing>) -> Session {
        Session {
            out,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Sender<Event>>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn send(&self, value: &Value) -> Result<(), CallError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(CallError::NotRunning("it exited".into()));
        }
        match self.out.try_send(Outgoing::Line(value.to_string())) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(CallError::Died("it stopped reading".into())),
            Err(TrySendError::Disconnected(_)) => Err(CallError::NotRunning("it exited".into())),
        }
    }

    /// Send a request and wait for its answer, passing along any `progress`
    /// the plugin reports for it meanwhile.
    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        mut on_progress: impl FnMut(&str),
    ) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending().insert(id, tx);
        // `fail_all` may have run between the liveness check in `send` and
        // the insert above; check again so the call can't wait for nothing.
        let sent = self
            .send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .and_then(|()| {
                if self.alive.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(CallError::NotRunning("it exited".into()))
                }
            });
        if let Err(e) = sent {
            self.pending().remove(&id);
            return Err(e);
        }
        let deadline = Instant::now() + timeout;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Event::Progress(message)) => on_progress(&message),
                Ok(Event::Done(result)) => return result,
                Err(RecvTimeoutError::Timeout) => {
                    self.pending().remove(&id);
                    return Err(CallError::Timeout(timeout));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CallError::Died("it went away".into()))
                }
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), CallError> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// The plugin is gone: fail every call waiting on it.
    fn fail_all(&self, why: &str) {
        self.alive.store(false, Ordering::SeqCst);
        let waiting: Vec<_> = self.pending().drain().collect();
        for (_, tx) in waiting {
            let _ = tx.send(Event::Done(Err(CallError::Died(why.to_string()))));
        }
    }

    fn complete(&self, id: u64, result: Result<Value, CallError>) {
        if let Some(tx) = self.pending().remove(&id) {
            let _ = tx.send(Event::Done(result));
        }
    }

    fn progress(&self, id: u64, message: String) {
        if let Some(tx) = self.pending().get(&id) {
            let _ = tx.send(Event::Progress(message));
        }
    }
}

// ---------------------------------------------------------------------------
// The plugin's log

/// `~/.tome/plugins/<name>/plugin.log`, appended to by the host and by the
/// plugin's stderr.
#[derive(Clone)]
pub struct Log {
    file: Arc<Mutex<Option<File>>>,
}

impl Log {
    pub fn open(plugin: &str) -> Log {
        let path = store::log_path(plugin);
        let _ = fs::create_dir_all(path.parent().unwrap_or(&path));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        Log {
            file: Arc::new(Mutex::new(file)),
        }
    }

    pub fn line(&self, source: &str, text: &str) {
        let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
        if let Some(file) = self.file.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            let _ = writeln!(file, "{stamp} [{source}] {text}");
        }
    }
}

// ---------------------------------------------------------------------------
// Starting a plugin process

/// What a plugin may ask of tome with a request of its own. The only request
/// today is `notify {title, message}`.
pub fn plugin_request(plugin: &str, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "notify" => {
            let text = |k: &str| {
                params
                    .get(k)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            };
            let (Some(title), Some(message)) = (text("title"), text("message")) else {
                return Err((-32602, "`notify` needs a `title` and a `message`".into()));
            };
            crate::orchestrator::notify_delivery(&format!("{plugin}: {title}"), message);
            Ok(json!({}))
        }
        other => Err((-32601, format!("unknown method `{other}`"))),
    }
}

struct Running {
    child: Child,
    session: Arc<Session>,
}

/// Start the plugin's process and wire up its pipes. Doesn't talk to it.
fn spawn(plugin: &Installed, manifest: &Manifest, log: &Log) -> Result<Running, String> {
    let rel = manifest
        .command
        .as_ref()
        .ok_or("the manifest has no `command` to start")?;
    let path = plugin.dir.join(rel);
    let data = plugin.data_dir();
    fs::create_dir_all(&data).map_err(|e| format!("creating {}: {e}", data.display()))?;
    let mut child = Command::new(&path)
        .current_dir(&data)
        .env("TOME_PLUGIN", &plugin.name)
        .env("TOME_PLUGIN_DIR", &plugin.dir)
        .env("TOME_PLUGIN_DATA", &data)
        .env_remove("TOME_RUN_ID")
        .env_remove("TOME_NODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group: a Ctrl-C meant for the daemon doesn't reach it.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", path.display()))?;

    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let (out, queued): (SyncSender<Outgoing>, Receiver<Outgoing>) = mpsc::sync_channel(QUEUE);
    let session = Arc::new(Session::new(out));

    std::thread::spawn(move || {
        let mut stdin = stdin;
        for item in queued {
            match item {
                Outgoing::Line(mut line) => {
                    line.push('\n');
                    if stdin
                        .write_all(line.as_bytes())
                        .and_then(|()| stdin.flush())
                        .is_err()
                    {
                        return;
                    }
                }
                Outgoing::Close => return,
            }
        }
    });
    std::thread::spawn({
        let log = log.clone();
        move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log.line("stderr", &line);
            }
        }
    });
    std::thread::spawn({
        let session = Arc::clone(&session);
        let log = log.clone();
        let name = plugin.name.clone();
        move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                read_line(&session, &name, &log, &line);
            }
            session.fail_all("its output closed");
        }
    });
    Ok(Running { child, session })
}

/// One line from the plugin: an answer to a call, a `progress`, or a request
/// of its own.
fn read_line(session: &Session, plugin: &str, log: &Log, line: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        log.line("stdout", line);
        return;
    };
    if let Some(method) = msg.get("method").and_then(Value::as_str) {
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match (method, msg.get("id")) {
            ("progress", None) => {
                let id = params.get("id").and_then(Value::as_u64);
                let message = params.get("message").and_then(Value::as_str);
                if let (Some(id), Some(message)) = (id, message) {
                    session.progress(id, message.to_string());
                }
            }
            (_, Some(id)) => {
                let reply = match plugin_request(plugin, method, &params) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err((code, message)) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": code, "message": message },
                    }),
                };
                let _ = session.send(&reply);
            }
            (other, None) => log.line("host", &format!("ignored notification `{other}`")),
        }
        return;
    }
    let Some(id) = msg.get("id").and_then(Value::as_u64) else {
        log.line("stdout", line);
        return;
    };
    let result = match msg.get("error") {
        Some(err) => Err(CallError::Plugin {
            code: err.get("code").and_then(Value::as_i64).unwrap_or(-32000),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the plugin reported an error")
                .to_string(),
            data: err.get("data").cloned(),
        }),
        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
    };
    session.complete(id, result);
}

/// How `initialize` ended.
enum Init {
    Ready,
    /// The plugin says it can't work with this tome.
    Incompatible(String),
    Failed(String),
}

fn initialize(plugin: &Installed, session: &Session) -> Init {
    let params = json!({
        "protocol": PROTOCOL,
        "tome_version": store::tome_version().to_string(),
        "plugin": plugin.name,
        "state_dir": plugin.data_dir(),
    });
    match session.request("initialize", params, INIT_TIMEOUT, |_| {}) {
        Ok(reply) => match reply.get("protocol").and_then(Value::as_u64) {
            Some(p) if p != u64::from(PROTOCOL) => Init::Incompatible(format!(
                "the plugin answered `initialize` with protocol {p}; this tome speaks {PROTOCOL}"
            )),
            _ => Init::Ready,
        },
        Err(CallError::Plugin { message, data, .. })
            if data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str)
                == Some("incompatible") =>
        {
            Init::Incompatible(message)
        }
        Err(e) => Init::Failed(format!("`initialize` failed: {e}")),
    }
}

/// Close a plugin's stdin and give it `STOP_GRACE` to exit, then kill it.
fn stop_child(running: &mut Running) {
    let _ = running.session.notify("shutdown", json!({}));
    let _ = running.session.out.try_send(Outgoing::Close);
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(running.child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = running.child.kill();
    let _ = running.child.wait();
}

// ---------------------------------------------------------------------------
// The check `tome plugin check` makes

/// Start the plugin, run `initialize`, and stop it again.
pub fn check_start(plugin: &Installed, manifest: &Manifest) -> Result<(), String> {
    let log = Log::open(&plugin.name);
    log.line("host", "plugin check: starting");
    let mut running = spawn(plugin, manifest, &log)?;
    let verdict = match initialize(plugin, &running.session) {
        Init::Ready => Ok(()),
        Init::Incompatible(why) => Err(format!("the plugin refused `initialize`: {why}")),
        Init::Failed(why) => Err(why),
    };
    // A plugin that died during the handshake says why on stderr.
    stop_child(&mut running);
    running.session.fail_all("checked");
    log.line("host", &format!("plugin check: {verdict:?}"));
    verdict
}

// ---------------------------------------------------------------------------
// The host

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum State {
    Starting,
    Running,
    Restarting { failures: u32, error: String },
    Failed { error: String },
    Incompatible { error: String },
    Stopped,
}

impl State {
    pub fn word(&self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Running => "running",
            State::Restarting { .. } => "restarting",
            State::Failed { .. } => "failed",
            State::Incompatible { .. } => "incompatible",
            State::Stopped => "stopped",
        }
    }
}

struct Slot {
    name: String,
    /// What it was started from: a changed manifest restarts it.
    manifest: Manifest,
    hooks: Vec<Hook>,
    state: Mutex<State>,
    session: Mutex<Option<Arc<Session>>>,
    pid: Mutex<Option<u32>>,
    stop: Stop,
    log: Log,
    last_notified: Mutex<HashMap<String, Instant>>,
    /// `daemon.started` has been sent: it is sent once, not on each restart.
    announced: AtomicBool,
}

impl Slot {
    fn set(&self, state: State) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = state;
    }

    fn state(&self) -> State {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn hook(&self, name: &str) -> Option<&Hook> {
        self.hooks.iter().find(|h| h.name == name)
    }

    fn session(&self) -> Result<Arc<Session>, CallError> {
        let state = self.state();
        if state != State::Running {
            return Err(CallError::NotRunning(state.word().to_string()));
        }
        self.session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| CallError::NotRunning("it isn't connected".into()))
    }
}

/// The plugin behind a provider name, and its timeouts.
#[derive(Debug, Clone)]
pub struct ProviderHooks {
    pub plugin: String,
    pub provide: Duration,
    pub release: Duration,
}

/// What `plugin.status` reports for one plugin.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub name: String,
    #[serde(flatten)]
    pub state: State,
    pub pid: Option<u32>,
}

/// The daemon's plugins. Cheap to create empty: [`Host::start`] begins
/// running whatever is installed.
#[derive(Default)]
pub struct Host {
    slots: Mutex<Vec<Arc<Slot>>>,
    closed: AtomicBool,
}

impl Host {
    pub fn new() -> Host {
        Host::default()
    }

    fn slots(&self) -> Vec<Arc<Slot>> {
        self.slots.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn slot(&self, name: &str) -> Option<Arc<Slot>> {
        self.slots().into_iter().find(|s| s.name == name)
    }

    /// Start every enabled, compatible plugin that has hooks, and stop
    /// those that no longer are (after `enable`, `disable`, `rm`, `update`).
    pub fn reload(self: &Arc<Self>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let wanted: Vec<(Installed, Manifest)> = store::active()
            .into_iter()
            .filter_map(|p| {
                let m = p.active()?.clone();
                (m.command.is_some() && !m.hooks.is_empty()).then_some((p, m))
            })
            .collect();
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        // Drop the ones that are gone or changed.
        slots.retain(|slot| {
            let keep = wanted
                .iter()
                .any(|(p, m)| p.name == slot.name && *m == slot.manifest);
            if !keep {
                slot.stop.set();
            }
            keep
        });
        for (plugin, manifest) in wanted {
            if slots.iter().any(|s| s.name == plugin.name) {
                continue;
            }
            let slot = Arc::new(Slot {
                name: plugin.name.clone(),
                manifest: manifest.clone(),
                hooks: manifest.hooks.clone(),
                state: Mutex::new(State::Starting),
                session: Mutex::new(None),
                pid: Mutex::new(None),
                stop: Stop::default(),
                log: Log::open(&plugin.name),
                last_notified: Mutex::new(HashMap::new()),
                announced: AtomicBool::new(false),
            });
            slots.push(Arc::clone(&slot));
            std::thread::spawn(move || supervise(&slot, &plugin, &manifest));
        }
    }

    pub fn status(&self) -> Vec<Status> {
        self.slots()
            .iter()
            .map(|s| Status {
                name: s.name.clone(),
                state: s.state(),
                pid: *s.pid.lock().unwrap_or_else(|p| p.into_inner()),
            })
            .collect()
    }

    /// The plugin that provides machines named `provider`, with how long
    /// `provide` and `release` may take.
    pub fn provider(&self, provider: &str) -> Option<ProviderHooks> {
        self.slots().iter().find_map(|s| {
            s.manifest
                .providers
                .iter()
                .any(|p| p == provider)
                .then(|| ProviderHooks {
                    plugin: s.name.clone(),
                    provide: s
                        .hook("provide")
                        .and_then(|h| h.timeout)
                        .unwrap_or(PROVIDE_TIMEOUT),
                    release: s
                        .hook("release")
                        .and_then(|h| h.timeout)
                        .unwrap_or(RELEASE_TIMEOUT),
                })
        })
    }

    /// The plugins declaring `hook`, running or not, with the hook's options.
    pub fn declaring(&self, hook: &str) -> Vec<(String, Hook)> {
        self.slots()
            .iter()
            .filter_map(|s| s.hook(hook).map(|h| (s.name.clone(), h.clone())))
            .collect()
    }

    /// Call `method` on `plugin` and wait for the answer, up to `timeout`.
    pub fn call(
        &self,
        plugin: &str,
        method: &str,
        params: Value,
        timeout: Duration,
        on_progress: impl FnMut(&str),
    ) -> Result<Value, CallError> {
        let slot = self
            .slot(plugin)
            .ok_or_else(|| CallError::NotRunning("it isn't started".into()))?;
        let session = slot.session()?;
        let result = session.request(method, params, timeout, on_progress);
        if let Err(e) = &result {
            slot.log.line("host", &format!("{method} failed: {e}"));
        }
        result
    }

    /// Tell every plugin declaring `hook` about an event, without waiting:
    /// each gets the request on a thread of its own, and a failure is logged
    /// and notified, never raised.
    pub fn fire(self: &Arc<Self>, hook: &'static str, params: Value) {
        for (plugin, options) in self.declaring(hook) {
            let host = Arc::clone(self);
            let params = params.clone();
            std::thread::spawn(move || {
                let timeout = options.timeout.unwrap_or(HOOK_TIMEOUT);
                if let Err(e) = host.call(&plugin, hook, params, timeout, |_| {}) {
                    host.report_failure(&plugin, hook, &e);
                }
            });
        }
    }

    /// A hook that failed: notify, at most once a minute per plugin and hook,
    /// unless the plugin is down (that was notified when it failed).
    fn report_failure(&self, plugin: &str, hook: &str, error: &CallError) {
        if let Some(slot) = self.slot(plugin) {
            report_failure(&slot, hook, error);
        }
    }

    /// Stop the plugins: tell the ones that want to know (`daemon.stopping`,
    /// answered within a short grace), then close their pipes and kill what
    /// doesn't leave.
    pub fn shutdown(self: &Arc<Self>) {
        self.closed.store(true, Ordering::SeqCst);
        let waiting: Vec<_> = self
            .declaring("daemon.stopping")
            .into_iter()
            .map(|(plugin, options)| {
                let host = Arc::clone(self);
                std::thread::spawn(move || {
                    let timeout = options.timeout.unwrap_or(HOOK_TIMEOUT).min(STOP_GRACE);
                    let _ = host.call(&plugin, "daemon.stopping", json!({}), timeout, |_| {});
                })
            })
            .collect();
        for w in waiting {
            let _ = w.join();
        }
        for slot in self.slots() {
            slot.stop.set();
        }
        // The supervisors stop their children; wait briefly for them.
        let deadline = Instant::now() + STOP_GRACE + Duration::from_secs(1);
        while Instant::now() < deadline
            && self
                .slots()
                .iter()
                .any(|s| s.pid.lock().unwrap_or_else(|p| p.into_inner()).is_some())
        {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A hook that failed: notify, at most once a minute per plugin and hook,
/// unless the plugin is down (that was notified when it failed).
fn report_failure(slot: &Slot, hook: &str, error: &CallError) {
    if matches!(error, CallError::NotRunning(_)) {
        return;
    }
    let mut last = slot.last_notified.lock().unwrap_or_else(|p| p.into_inner());
    if last.get(hook).is_some_and(|t| t.elapsed() < NOTIFY_EVERY) {
        return;
    }
    last.insert(hook.to_string(), Instant::now());
    crate::orchestrator::notify_delivery(
        &format!("plugin {} failed", slot.name),
        &format!(
            "hook `{hook}`: {error} (see {})",
            store::log_path(&slot.name).display()
        ),
    );
}

/// Tell a plugin the daemon is up, once, without holding anything up.
fn announce_started(slot: &Arc<Slot>, session: &Arc<Session>) {
    let Some(hook) = slot.hook("daemon.started") else {
        return;
    };
    if slot.announced.swap(true, Ordering::SeqCst) {
        return;
    }
    let (slot, session) = (Arc::clone(slot), Arc::clone(session));
    let timeout = hook.timeout.unwrap_or(HOOK_TIMEOUT);
    std::thread::spawn(move || {
        let params = json!({
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "socket": crate::paths::socket_path(),
            "home": crate::paths::tome_home(),
        });
        if let Err(e) = session.request("daemon.started", params, timeout, |_| {}) {
            slot.log
                .line("host", &format!("daemon.started failed: {e}"));
            report_failure(&slot, "daemon.started", &e);
        }
    });
}

/// Run one plugin for as long as it's wanted: start it, restart it when it
/// dies, give up when it keeps failing.
fn supervise(slot: &Arc<Slot>, plugin: &Installed, manifest: &Manifest) {
    let mut failures = 0u32;
    loop {
        if slot.stop.is_set() {
            break;
        }
        slot.set(State::Starting);
        let started = Instant::now();
        let error = match spawn(plugin, manifest, &slot.log) {
            Err(e) => e,
            Ok(mut running) => {
                *slot.pid.lock().unwrap_or_else(|p| p.into_inner()) = Some(running.child.id());
                let outcome = match initialize(plugin, &running.session) {
                    Init::Ready => {
                        *slot.session.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(Arc::clone(&running.session));
                        slot.set(State::Running);
                        slot.log.line("host", "running");
                        announce_started(slot, &running.session);
                        wait_for_exit(slot, &mut running)
                    }
                    Init::Incompatible(why) => Outcome::Incompatible(why),
                    Init::Failed(why) => Outcome::Crashed(why),
                };
                *slot.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
                running.session.fail_all("it exited");
                stop_child(&mut running);
                *slot.pid.lock().unwrap_or_else(|p| p.into_inner()) = None;
                match outcome {
                    Outcome::Stopped => break,
                    Outcome::Incompatible(why) => {
                        slot.log.line("host", &format!("incompatible: {why}"));
                        slot.set(State::Incompatible { error: why });
                        return;
                    }
                    Outcome::Crashed(why) => why,
                }
            }
        };
        slot.log.line("host", &format!("down: {error}"));
        if started.elapsed() >= STABLE_AFTER {
            failures = 0;
        }
        failures += 1;
        if failures >= MAX_FAILURES {
            slot.set(State::Failed {
                error: error.clone(),
            });
            crate::orchestrator::notify_delivery(
                &format!("plugin {} failed", slot.name),
                &format!(
                    "gave up after {failures} failed starts: {error} (see {})",
                    store::log_path(&slot.name).display()
                ),
            );
            return;
        }
        slot.set(State::Restarting {
            failures,
            error: error.clone(),
        });
        if !slot.stop.sleep(backoff(failures)) {
            break;
        }
    }
    slot.set(State::Stopped);
}

enum Outcome {
    Stopped,
    Incompatible(String),
    Crashed(String),
}

/// Block until the plugin's process exits or the slot is stopped.
fn wait_for_exit(slot: &Slot, running: &mut Running) -> Outcome {
    loop {
        match running.child.try_wait() {
            Ok(Some(status)) => return Outcome::Crashed(format!("it exited ({status})")),
            Ok(None) => {}
            Err(e) => return Outcome::Crashed(format!("waiting for it: {e}")),
        }
        if !slot.stop.sleep(Duration::from_millis(100)) {
            return Outcome::Stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        std::env::set_var("TOME_PLUGIN_BACKOFF_MS", "100");
        assert_eq!(backoff(1), Duration::from_millis(100));
        assert_eq!(backoff(2), Duration::from_millis(200));
        assert_eq!(backoff(3), Duration::from_millis(400));
        assert_eq!(backoff(40), Duration::from_millis(3200));
        std::env::remove_var("TOME_PLUGIN_BACKOFF_MS");
    }

    fn pair() -> (Arc<Session>, Receiver<Outgoing>) {
        let (tx, rx) = mpsc::sync_channel(8);
        (Arc::new(Session::new(tx)), rx)
    }

    fn sent(rx: &Receiver<Outgoing>) -> Value {
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Outgoing::Line(l) => serde_json::from_str(&l).unwrap(),
            Outgoing::Close => panic!("closed"),
        }
    }

    #[test]
    fn a_call_gets_its_answer_and_progress() {
        let (session, rx) = pair();
        let log = Log::open("unit-test-nonexistent");
        let s = Arc::clone(&session);
        let caller = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let r = s.request("provide", json!({"a": 1}), Duration::from_secs(5), |m| {
                seen.push(m.to_string())
            });
            (r, seen)
        });
        let req = sent(&rx);
        assert_eq!(req["method"], "provide");
        let id = req["id"].as_u64().unwrap();
        read_line(
            &session,
            "x",
            &log,
            &json!({"jsonrpc":"2.0","method":"progress","params":{"id":id,"message":"booting"}})
                .to_string(),
        );
        read_line(
            &session,
            "x",
            &log,
            &json!({"jsonrpc":"2.0","id":id,"result":{"ssh":"me@h"}}).to_string(),
        );
        let (r, seen) = caller.join().unwrap();
        assert_eq!(r.unwrap()["ssh"], "me@h");
        assert_eq!(seen, vec!["booting"]);
    }

    #[test]
    fn a_dead_plugin_fails_calls_in_flight_and_after() {
        let (session, rx) = pair();
        let s = Arc::clone(&session);
        let caller =
            std::thread::spawn(move || s.request("x", json!({}), Duration::from_secs(30), |_| {}));
        let _ = sent(&rx);
        session.fail_all("it exited");
        assert!(matches!(caller.join().unwrap(), Err(CallError::Died(_))));
        assert!(matches!(
            session.request("y", json!({}), Duration::from_secs(1), |_| {}),
            Err(CallError::NotRunning(_))
        ));
    }

    #[test]
    fn a_hung_plugin_times_the_call_out() {
        let (session, _rx) = pair();
        let started = Instant::now();
        let r = session.request("x", json!({}), Duration::from_millis(50), |_| {});
        assert!(matches!(r, Err(CallError::Timeout(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(session.pending().is_empty(), "the call is forgotten");
    }

    #[test]
    fn a_plugin_error_carries_its_message_and_data() {
        let (session, rx) = pair();
        let log = Log::open("unit-test-nonexistent");
        let s = Arc::clone(&session);
        let caller =
            std::thread::spawn(move || s.request("x", json!({}), Duration::from_secs(5), |_| {}));
        let id = sent(&rx)["id"].as_u64().unwrap();
        read_line(
            &session,
            "x",
            &log,
            &json!({"jsonrpc":"2.0","id":id,"error":{"code":-1,"message":"nope","data":{"k":1}}})
                .to_string(),
        );
        match caller.join().unwrap() {
            Err(CallError::Plugin {
                code,
                message,
                data,
            }) => {
                assert_eq!((code, message.as_str()), (-1, "nope"));
                assert_eq!(data.unwrap()["k"], 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plugin_requests_are_checked() {
        assert_eq!(
            plugin_request("p", "notify", &json!({"title": "t"}))
                .unwrap_err()
                .0,
            -32602
        );
        assert_eq!(
            plugin_request("p", "other", &json!({})).unwrap_err().0,
            -32601
        );
    }
}
