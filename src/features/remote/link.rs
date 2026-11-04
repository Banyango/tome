//! The run's side: one link per node with remote workers on it. A link is
//! a thread that owns the node's `rpc::Client`. It loops on `hosted.poll`,
//! answers the calls the node relays from its hosted workers, ends workers
//! whose sessions ended, and runs the commands queued for it in between.
//! It closes once no running worker is on its node.

use super::{CAPABILITY, NODE_UNREACHABLE};
use crate::engine::Engine;
use crate::features::nodes::{self, Node};
use crate::handshake;
use crate::ids::RunId;
use crate::output::{CliError, CliResult, ErrorKind};
use crate::rpc::{Client, Response};
use crate::store::WorkerStatus;
use crate::workers;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a poll waits on the node for something to happen.
const POLL_WAIT_MS: u64 = 500;
/// A poll that takes longer than this means the link is gone.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long an idle link stays open with no workers on its node.
const LINGER: Duration = Duration::from_secs(10);
/// How long a call over a link waits for its answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Something for a link's thread to do with its client.
type Cmd = Box<dyn FnOnce(&mut Client) + Send>;

/// The calls a hosted worker may make through its link.
fn relayable(method: &str) -> bool {
    workers::METHODS.contains(&method)
        || matches!(
            method,
            "step.report" | "step.status" | "agent.ready" | "agent.blocked"
        )
}

pub struct Links {
    /// Who this daemon is to the nodes.
    origin: String,
    open: Mutex<HashMap<String, Sender<Cmd>>>,
}

impl Default for Links {
    fn default() -> Links {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let salt = (nanos as u64) ^ (u64::from(std::process::id()) << 32);
        Links {
            origin: format!("{}-{salt:016x}", nodes::host_name()),
            open: Mutex::new(HashMap::new()),
        }
    }
}

impl Links {
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Queue `cmd` on `node`'s link, if it has one open.
    fn send(&self, node: &str, cmd: Cmd) -> bool {
        let open = self.open.lock().unwrap_or_else(|p| p.into_inner());
        open.get(node).is_some_and(|tx| tx.send(cmd).is_ok())
    }

    fn forget(&self, node: &str) {
        self.open
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(node);
    }
}

/// Whether a node's `daemon.ping` says it can host workers.
pub fn can_host(pong: &Value) -> bool {
    pong["capabilities"]
        .as_array()
        .is_some_and(|c| c.iter().any(|v| v == CAPABILITY))
}

impl Engine {
    /// Call `method` on `node`, over its link (opened if need be).
    pub(crate) fn node_call(&self, node: &Node, method: &str, params: Value) -> CliResult<Value> {
        let method = method.to_string();
        self.with_node(node, move |client| client.call(&method, params))
    }

    /// Run `f` with `node`'s client, on its link's thread (the link is
    /// opened if need be), and wait for what it returns.
    pub(crate) fn with_node<T: Send + 'static>(
        &self,
        node: &Node,
        f: impl FnOnce(&mut Client) -> CliResult<T> + Send + 'static,
    ) -> CliResult<T> {
        self.open_link(node)?;
        let (tx, rx) = mpsc::sync_channel(1);
        let sent = self.links.send(
            &node.name,
            Box::new(move |client: &mut Client| {
                let _ = tx.send(f(client));
            }),
        );
        if !sent {
            return Err(lost(&node.name, "the link closed"));
        }
        rx.recv_timeout(CALL_TIMEOUT)
            .map_err(|_| lost(&node.name, "it didn't answer"))?
    }

    /// Tell `node` to close a run's hosted worker (or all of the run's),
    /// if its link is open. Doesn't wait.
    pub(crate) fn node_close(&self, node: &str, run_id: RunId, name: Option<&str>, kill: bool) {
        let params = json!({
            "origin": self.links.origin(),
            "run_id": run_id.get(),
            "name": name,
            "kill": kill,
        });
        self.links.send(
            node,
            Box::new(move |client: &mut Client| {
                let _ = client.call("hosted.close", params);
            }),
        );
    }

    /// Open `node`'s link unless it's open: connect, check the node can
    /// host workers, and start the link's thread.
    pub(crate) fn open_link(&self, node: &Node) -> CliResult<()> {
        if self
            .links
            .open
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&node.name)
        {
            return Ok(());
        }
        let client = Client::to_node(node)?;
        if !client.pong.as_ref().is_some_and(can_host) {
            return Err(CliError::invalid(format!(
                "node {} can't host workers: its tome is too old",
                node.name
            ))
            .with_hint(format!("upgrade tome on {}", node.name)));
        }
        let Some(engine) = self.me() else {
            return Err(CliError::internal("the daemon isn't running links"));
        };
        let mut open = self.links.open.lock().unwrap_or_else(|p| p.into_inner());
        if open.contains_key(&node.name) {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        open.insert(node.name.clone(), tx);
        let name = node.name.clone();
        std::thread::spawn(move || engine.run_link(&name, client, rx));
        Ok(())
    }

    fn run_link(self: Arc<Self>, node: &str, mut client: Client, rx: Receiver<Cmd>) {
        let (reply_tx, reply_rx) = mpsc::channel::<Value>();
        let inflight = Arc::new(AtomicUsize::new(0));
        let mut busy_at = std::time::Instant::now();
        loop {
            let replies: Vec<Value> = reply_rx.try_iter().collect();
            let params = json!({
                "origin": self.links.origin(),
                "wait_ms": POLL_WAIT_MS,
                "replies": replies,
            });
            let polled = match client.call_within("hosted.poll", params, POLL_TIMEOUT) {
                Ok(v) => v,
                Err(e) => return self.link_lost(node, &e.message),
            };
            let calls = polled["calls"].as_array().cloned().unwrap_or_default();
            for call in &calls {
                inflight.fetch_add(1, Ordering::SeqCst);
                let (engine, call, tx, inflight) = (
                    Arc::clone(&self),
                    call.clone(),
                    reply_tx.clone(),
                    inflight.clone(),
                );
                let node = node.to_string();
                std::thread::spawn(move || {
                    let _ = tx.send(engine.relayed(&node, &call));
                    inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            for ended in polled["ended"].as_array().into_iter().flatten() {
                self.remote_ended(node, ended);
            }
            while let Ok(cmd) = rx.try_recv() {
                cmd(&mut client);
            }
            if self.stop.is_set() {
                return self.links.forget(node);
            }
            let busy =
                !replies.is_empty() || !calls.is_empty() || inflight.load(Ordering::SeqCst) > 0;
            if busy {
                busy_at = std::time::Instant::now();
            }
            if busy_at.elapsed() < LINGER {
                continue;
            }
            // Idle: close, unless a worker is still on the node. Commands
            // are sent with the map locked, so none can slip in.
            let mut open = self.links.open.lock().unwrap_or_else(|p| p.into_inner());
            if let Ok(cmd) = rx.try_recv() {
                drop(open);
                cmd(&mut client);
                continue;
            }
            let running = self
                .with_store(|store| store.running_workers_on(node))
                .map(|w| !w.is_empty())
                .unwrap_or(true);
            if !running {
                open.remove(node);
                return;
            }
        }
    }

    /// Answer a call a hosted worker made, as a JSON-RPC response.
    fn relayed(&self, node: &str, call: &Value) -> Value {
        let id = call["id"].clone();
        match self.answer_relayed(node, call) {
            Ok(v) => json!(Response::ok(id, v)),
            Err(e) => json!(Response::from_cli_error(id, &e)),
        }
    }

    fn answer_relayed(&self, node: &str, call: &Value) -> CliResult<Value> {
        let method = call["method"].as_str().unwrap_or_default();
        if !relayable(method) {
            return Err(CliError::invalid(format!(
                "`{method}` can't be called from a remote worker"
            )));
        }
        let Some((run_id, Some(name))) = handshake::caller_of(&call["caller"]) else {
            return Err(CliError::invalid("a remote call must come from a worker"));
        };
        let params = &call["params"];
        if params.get("run_id").is_some_and(|r| {
            r.as_i64() != Some(run_id.get()) && r.as_str() != Some(&run_id.to_string())
        }) {
            return Err(CliError::invalid(
                "a remote worker can only act on its own run",
            ));
        }
        let w = self.with_store(|store| store.require_worker(run_id, &name))?;
        if w.node.as_deref() != Some(node) {
            return Err(CliError::invalid(format!(
                "worker {name} of run {run_id} isn't on {node}"
            )));
        }
        if w.status.is_final() {
            return Err(CliError::invalid(format!(
                "worker {name} already ended ({})",
                w.status.as_str()
            )));
        }
        self.seen((run_id, Some(name)));
        self.dispatch(method, params)
    }

    /// A hosted worker's session ended without a report reaching us.
    fn remote_ended(&self, node: &str, ended: &Value) {
        let (Some(run_id), Some(name)) = (ended["run_id"].as_i64(), ended["name"].as_str()) else {
            return;
        };
        let Ok(Some(w)) = self.with_store(|store| store.worker(RunId::new(run_id), name)) else {
            return;
        };
        if w.node.as_deref() != Some(node) || w.status.is_final() {
            return;
        }
        let exit = ended["exit_code"].as_i64().map(|c| c as i32);
        self.end_exited(&w, exit, ended["tail"].as_str().unwrap_or_default());
    }

    /// The link to `node` broke: its running workers fail now, so the
    /// orchestrator can respawn them elsewhere.
    fn link_lost(&self, node: &str, why: &str) {
        self.links.forget(node);
        let Ok(workers) = self.with_store(|store| store.running_workers_on(node)) else {
            return;
        };
        eprintln!("tome daemon: lost the link to node {node}: {why}");
        let summary = format!("lost the link to node {node}: {why}");
        for w in workers {
            let end = self.with_store(|store| {
                let end = store.finish_worker(
                    w.run_id,
                    &w.name,
                    WorkerStatus::Failed,
                    Some(NODE_UNREACHABLE),
                    Some(&summary),
                    None,
                    true,
                )?;
                self.sync(store, w.run_id);
                Ok(end)
            });
            if let Ok(end) = end {
                self.after_end(w.run_id, &end, None);
            }
        }
    }

    /// Close every hosted worker of a run on the nodes it used.
    pub(crate) fn close_remote(&self, run_id: RunId) {
        let Ok(workers) = self.with_store(|store| store.workers(run_id)) else {
            return;
        };
        let mut nodes: Vec<String> = workers.into_iter().filter_map(|w| w.node).collect();
        nodes.sort();
        nodes.dedup();
        for node in nodes {
            self.node_close(&node, run_id, None, true);
        }
    }
}

fn lost(node: &str, why: &str) -> CliError {
    CliError::new(
        ErrorKind::Unreachable,
        format!("lost the link to node {node}: {why}"),
    )
    .with_hint(format!("check it with `tome node check {node}`"))
}
