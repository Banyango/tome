//! How workflow errors reach the CLI: the conversions from the workflow
//! module's own error types to [`CliError`], with the exit kinds and hints the
//! output contract needs. `workflow` itself knows nothing of `output`.

use crate::output::{CliError, ErrorKind};
use crate::workflow::{FindError, Invalid, LibraryError, ParamArgError};
use serde_json::json;

impl Invalid {
    pub fn into_cli_error(self) -> CliError {
        let label = self
            .name
            .clone()
            .unwrap_or_else(|| self.path.display().to_string());
        let n = self.errors.len();
        CliError::new(
            ErrorKind::InvalidWorkflow,
            format!(
                "workflow `{label}` is invalid ({n} error{})",
                if n == 1 { "" } else { "s" }
            ),
        )
        .with_details(json!({ "path": self.path, "errors": self.errors_json() }))
    }
}

impl From<Invalid> for CliError {
    fn from(inv: Invalid) -> Self {
        inv.into_cli_error()
    }
}

impl From<ParamArgError> for CliError {
    fn from(e: ParamArgError) -> Self {
        CliError::invalid(e.to_string())
    }
}

impl From<LibraryError> for CliError {
    fn from(e: LibraryError) -> Self {
        let message = e.to_string();
        match e {
            LibraryError::NoFileAt { .. } => CliError::not_found(message),
            LibraryError::DuplicateWithPlugin { .. } => {
                CliError::new(ErrorKind::InvalidWorkflow, message).with_hint(
                    "rename one, or disable the plugin with `tome plugin disable <name>`",
                )
            }
            LibraryError::Duplicate { .. } => CliError::new(ErrorKind::InvalidWorkflow, message),
            LibraryError::NotFound {
                global_dir,
                project_dir,
                ..
            } => CliError::not_found(message).with_hint(format!(
                "workflows are loaded from {}{}",
                global_dir.display(),
                project_dir
                    .map(|d| format!(" and {}", d.display()))
                    .unwrap_or_default()
            )),
        }
    }
}

impl From<FindError> for CliError {
    fn from(e: FindError) -> Self {
        match e {
            FindError::Library(e) => e.into(),
            FindError::Invalid(inv) => inv.into(),
        }
    }
}
