//! Spawning a worker on a node: the run's side of `worker.spawn --node`.

use crate::engine::Engine;
use crate::features::nodes::{self, Node};
use crate::orchestrator;
use crate::output::{CliError, CliResult};
use crate::store::Run;
use serde_json::{json, Value};

/// What a remote worker runs.
pub struct RemoteTask<'a> {
    pub name: &'a str,
    pub command: &'a [String],
    pub keep_open: bool,
    /// The placement flags it was spawned with.
    pub placement: Option<&'a Value>,
}

impl Engine {
    /// The node `name` names, connected and able to host workers.
    pub(crate) fn remote_node(&self, name: &str) -> CliResult<Node> {
        if self.plugins.provider(name).is_some() {
            return Err(CliError::invalid(format!(
                "`{name}` is a provider, not a node: remote workers need a configured node"
            ))
            .with_hint("add one with `tome node add <name> <ssh-destination>`"));
        }
        let node = nodes::get(name)?;
        self.open_link(&node)?;
        Ok(node)
    }

    /// Start a worker's session on `node`. Returns its session's name.
    pub(crate) fn launch_remote(
        &self,
        run: &Run,
        node: &Node,
        task: &RemoteTask,
    ) -> CliResult<String> {
        let project = match orchestrator::run_project(run) {
            Some(local) => {
                let n = node.clone();
                let located =
                    self.with_node(node, move |client| nodes::locate(client, &n, &local, false))?;
                Some(located.path)
            }
            None => None,
        };
        let out = self.node_call(
            node,
            "hosted.spawn",
            json!({
                "origin": self.links.origin(),
                "run_id": run.id.get(),
                "name": task.name,
                "workflow": run.workflow_name,
                "command": task.command,
                "keep_open": task.keep_open,
                "project": project,
                "placement": task.placement,
            }),
        )?;
        out["session"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| CliError::internal(format!("{} didn't name the session", node.name)))
    }
}
