//! The output contract every command follows.
//!
//! * Human-readable text by default; `--json` or `TOME_OUTPUT=json` switches to
//!   stable machine output (a single JSON document on stdout).
//! * Errors are a [`CliError`] with a stable `kind`. In JSON mode they print as
//!   `{"error": {"kind", "message", "hint"?}}` on stdout; in human mode they go
//!   to stderr.
//! * Exit codes are part of the contract, see [`exit`].

use serde_json::{json, Value};
use std::fmt;

/// Stable process exit codes.
pub mod exit {
    pub const OK: i32 = 0;
    /// Unexpected/internal failure.
    pub const FAILURE: i32 = 1;
    /// Invalid input: bad arguments or an invalid workflow.
    pub const INVALID: i32 = 2;
    /// The daemon isn't running (also `tome daemon status` when stopped).
    pub const DAEMON_UNAVAILABLE: i32 = 3;
    /// The requested object (run, workflow, ...) doesn't exist.
    pub const NOT_FOUND: i32 = 4;
    /// An attached run ended cancelled (Ctrl-C or `tome run cancel`).
    pub const CANCELLED: i32 = 130;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Json,
}

impl Mode {
    pub fn resolve(json_flag: bool) -> Mode {
        let env_json = std::env::var("TOME_OUTPUT")
            .map(|v| v.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if json_flag || env_json {
            Mode::Json
        } else {
            Mode::Human
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Internal,
    Invalid,
    InvalidWorkflow,
    DaemonNotRunning,
    NotFound,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Internal => "internal",
            ErrorKind::Invalid => "invalid",
            ErrorKind::InvalidWorkflow => "invalid_workflow",
            ErrorKind::DaemonNotRunning => "daemon_not_running",
            ErrorKind::NotFound => "not_found",
        }
    }

    pub fn parse(s: &str) -> ErrorKind {
        match s {
            "invalid" => ErrorKind::Invalid,
            "invalid_workflow" => ErrorKind::InvalidWorkflow,
            "daemon_not_running" => ErrorKind::DaemonNotRunning,
            "not_found" => ErrorKind::NotFound,
            _ => ErrorKind::Internal,
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Internal => exit::FAILURE,
            ErrorKind::Invalid | ErrorKind::InvalidWorkflow => exit::INVALID,
            ErrorKind::DaemonNotRunning => exit::DAEMON_UNAVAILABLE,
            ErrorKind::NotFound => exit::NOT_FOUND,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
    pub hint: Option<String>,
    /// Extra structured detail (e.g. validation errors with line numbers).
    pub details: Option<Value>,
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CliError { kind, message: message.into(), hint: None, details: None }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn daemon_not_running() -> Self {
        Self::new(ErrorKind::DaemonNotRunning, "the tome daemon is not running").with_hint(
            "start it with `tome daemon start`, or register it as a service with `tome daemon install`",
        )
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn to_json(&self) -> Value {
        let mut err = json!({ "kind": self.kind.as_str(), "message": self.message });
        if let Some(hint) = &self.hint {
            err["hint"] = json!(hint);
        }
        if let Some(details) = &self.details {
            err["details"] = details.clone();
        }
        json!({ "error": err })
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        match err.downcast::<CliError>() {
            Ok(cli) => cli,
            Err(err) => CliError::internal(format!("{err:#}")),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(err: std::io::Error) -> Self {
        CliError::internal(err.to_string())
    }
}

pub type CliResult<T> = Result<T, CliError>;

/// What a command produced: machine-readable data plus its human rendering,
/// and the exit code to use (non-zero for "succeeded but reports a negative
/// state", like `daemon status` on a stopped daemon).
pub struct Report {
    pub data: Value,
    pub human: String,
    pub exit_code: i32,
    /// Already printed while the command ran (e.g. a streamed run).
    pub printed: bool,
}

impl Report {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        Report { data, human: human.into(), exit_code: exit::OK, printed: false }
    }

    /// Output that was already written as it happened; `emit` only sets the
    /// exit code.
    pub fn printed(data: Value, exit_code: i32) -> Self {
        Report { data, human: String::new(), exit_code, printed: true }
    }

    pub fn with_exit(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }
}

pub fn emit(mode: Mode, result: CliResult<Report>) -> i32 {
    match result {
        Ok(report) if report.printed => report.exit_code,
        Ok(report) => {
            match mode {
                Mode::Json => println!("{}", report.data),
                Mode::Human => {
                    if !report.human.is_empty() {
                        println!("{}", report.human.trim_end_matches('\n'));
                    }
                }
            }
            report.exit_code
        }
        Err(err) => {
            match mode {
                Mode::Json => println!("{}", err.to_json()),
                Mode::Human => {
                    eprintln!("error: {}", err.message);
                    if let Some(Value::Array(items)) = err.details.as_ref().and_then(|d| d.get("errors")) {
                        for item in items {
                            if let Some(text) = item.get("display").and_then(Value::as_str) {
                                eprintln!("  {text}");
                            }
                        }
                    }
                    if let Some(hint) = &err.hint {
                        eprintln!("hint: {hint}");
                    }
                }
            }
            err.kind.exit_code()
        }
    }
}

/// Render a left-aligned text table with a header row.
pub fn table(headers: &[&str], rows: Vec<Vec<String>>) -> String {
    let clean = |s: &str| s.replace(['\n', '\t'], " ");
    let rows: Vec<Vec<String>> = rows.into_iter().map(|r| r.iter().map(|c| clean(c)).collect()).collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let line = |cells: Vec<&str>| {
        let last = cells.len().saturating_sub(1);
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i == last {
                out.push_str(cell);
            } else {
                let pad = widths[i] - cell.chars().count();
                out.push_str(cell);
                out.push_str(&" ".repeat(pad + 2));
            }
        }
        out.trim_end().to_string() + "\n"
    };
    let mut out = line(headers.to_vec());
    for row in &rows {
        out.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns() {
        let t = table(&["ID", "NAME"], vec![vec!["1".into(), "alpha".into()], vec!["10".into(), "b\nc".into()]]);
        assert_eq!(t, "ID  NAME\n1   alpha\n10  b c\n");
    }

    #[test]
    fn error_json_shape_is_stable() {
        let err = CliError::daemon_not_running();
        let v = err.to_json();
        assert_eq!(v["error"]["kind"], "daemon_not_running");
        assert!(v["error"]["hint"].as_str().unwrap().contains("tome daemon start"));
        assert_eq!(err.kind.exit_code(), exit::DAEMON_UNAVAILABLE);
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [
            ErrorKind::Internal,
            ErrorKind::Invalid,
            ErrorKind::InvalidWorkflow,
            ErrorKind::DaemonNotRunning,
            ErrorKind::NotFound,
        ] {
            assert_eq!(ErrorKind::parse(kind.as_str()), kind);
        }
    }
}
