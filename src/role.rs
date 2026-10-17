//! How a run is carried out and the roles its sessions play. Shared by the
//! store, the session code and the orchestrator, so it depends on nothing.

use serde::Serialize;

/// How a workflow is run: by one agent that does the work itself, or by an
/// orchestrator that can delegate to workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Single,
    Orchestrated,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Single => "single",
            Mode::Orchestrated => "orchestrated",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "single" => Some(Mode::Single),
            "orchestrated" => Some(Mode::Orchestrated),
            _ => None,
        }
    }
}

/// A run's orchestrator.
pub const ROLE: &str = "orchestrator";
/// A single run's agent.
pub const AGENT_ROLE: &str = "agent";

/// The role of a run's main session in `mode`.
pub fn role(mode: Mode) -> &'static str {
    match mode {
        Mode::Single => AGENT_ROLE,
        Mode::Orchestrated => ROLE,
    }
}

/// Whether `role` is a run's main session: its orchestrator or its agent.
pub fn is_main(role: &str) -> bool {
    role == ROLE || role == AGENT_ROLE
}
