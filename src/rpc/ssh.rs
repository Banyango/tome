//! The SSH transport: `ssh -o BatchMode=yes -o ConnectTimeout=10 <dest>
//! <tome> rpc --stdio`, with the node's daemon at the other end. A thread
//! reads its stdout a line at a time, so a stream can wait with a timeout;
//! another collects its stderr to explain a connection that closes.

use super::Got;
use crate::node::Node;
use crate::output::{CliError, CliResult, ErrorKind};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What ssh exits with when it can't connect or authenticate.
const SSH_FAILED: i32 = 255;

/// What a shell exits with for a command it can't find.
const NOT_FOUND: i32 = 127;

pub struct Conn {
    node: Node,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<std::io::Result<Vec<u8>>>,
    stderr: Option<JoinHandle<String>>,
}

impl Conn {
    pub fn open(node: &Node) -> CliResult<Conn> {
        let mut child = node
            .ssh_command(&[])
            .arg(format!("{} rpc --stdio", node.tome()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Ctrl-C is for tome to handle (e.g. cancelling an attached
            // run over this connection), not for ssh to die of.
            .process_group(0)
            .spawn()
            .map_err(|e| {
                CliError::new(
                    ErrorKind::Unreachable,
                    format!(
                        "can't run `{}` to reach {}: {e}",
                        crate::node::ssh_bin(),
                        node.name
                    ),
                )
                .with_hint("install an OpenSSH client, or put `ssh` on the PATH")
            })?;
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Ok(Conn {
            node: node.clone(),
            stdin: child.stdin.take(),
            child,
            lines,
            stderr: Some(stderr),
        })
    }

    pub fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        stdin.write_all(line)?;
        stdin.flush()
    }

    pub fn read(&mut self, timeout: Duration) -> CliResult<Got> {
        match self.lines.recv_timeout(timeout) {
            Ok(Ok(line)) if line.ends_with(b"\n") => Ok(Got::Line(line)),
            // A last line without its newline: the connection closed mid-line.
            Ok(Ok(_)) | Err(RecvTimeoutError::Disconnected) => Ok(Got::Closed),
            Ok(Err(e)) => Err(CliError::new(
                ErrorKind::Unreachable,
                format!("lost the connection to {}: {e}", self.node.name),
            )
            .with_hint(self.node.check_hint())),
            Err(RecvTimeoutError::Timeout) => Ok(Got::Idle),
        }
    }

    /// Why the connection closed, from ssh's exit status and stderr.
    pub fn closed_error(&mut self, what: &str) -> CliError {
        self.stdin = None;
        let status = wait_for(&mut self.child, Duration::from_secs(5));
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        classify(&self.node, status.and_then(|s| s.code()), &stderr, what)
    }
}

impl Drop for Conn {
    /// Close our end and give ssh a moment to finish cleanly, so the node's
    /// relay sees a closed connection rather than a killed one.
    fn drop(&mut self) {
        self.stdin = None;
        if wait_for(&mut self.child, Duration::from_secs(2)).is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn wait_for(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => return None,
        }
    }
}

/// The error for an ssh that ended (exit `code`, with `stderr`) before
/// answering.
pub fn classify(node: &Node, code: Option<i32>, stderr: &str, what: &str) -> CliError {
    let said: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let last = said.last().copied().unwrap_or("");
    let missing = code == Some(NOT_FOUND)
        || said
            .iter()
            .any(|l| l.contains("command not found") || l.contains("No such file or directory"));
    if code == Some(SSH_FAILED) {
        let why = if last.is_empty() {
            "ssh failed".to_string()
        } else {
            last.to_string()
        };
        return CliError::new(
            ErrorKind::Unreachable,
            format!("can't reach {} ({}): {why}", node.name, node.ssh),
        )
        .with_hint(node.check_hint());
    }
    if missing {
        return CliError::new(
            ErrorKind::Unreachable,
            format!(
                "tome isn't found on {} as `{}` (in a non-interactive ssh shell): {last}",
                node.name,
                node.tome()
            ),
        )
        .with_hint(format!(
            "set `nodes.{}.tome` in ~/.tome/config.yaml to the full path of tome on {}, e.g. ~/.cargo/bin/tome",
            node.name, node.name
        ));
    }
    let detail = match (last.is_empty(), code) {
        (false, _) => format!(": {last}"),
        (true, Some(c)) => format!(" (exit {c})"),
        (true, None) => String::new(),
    };
    CliError::internal(format!(
        "the connection to {} closed {what}{detail}",
        node.name
    ))
    .with_hint(node.check_hint())
}

/// Whether ssh's complaint is about authentication.
pub fn is_auth_failure(e: &CliError) -> bool {
    e.kind == ErrorKind::Unreachable
        && (e.message.contains("Permission denied")
            || e.message.contains("Host key verification failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> Node {
        Node {
            name: "mini".into(),
            ssh: "kyle@mini".into(),
            tome: None,
            projects: Default::default(),
        }
    }

    #[test]
    fn ssh_failures_are_unreachable() {
        let e = classify(
            &node(),
            Some(255),
            "ssh: connect to host mini port 22: Connection refused\n",
            "x",
        );
        assert_eq!(e.kind, ErrorKind::Unreachable);
        assert_eq!(e.kind.exit_code(), 3);
        assert!(e.message.contains("Connection refused"), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some("run `tome node check mini`"));
        let auth = classify(
            &node(),
            Some(255),
            "kyle@mini: Permission denied (publickey).",
            "x",
        );
        assert!(is_auth_failure(&auth));
    }

    #[test]
    fn a_missing_tome_says_to_set_its_path() {
        let e = classify(&node(), Some(127), "sh: tome: command not found\n", "x");
        assert_eq!(e.kind, ErrorKind::Unreachable);
        assert!(e.hint.unwrap().contains("nodes.mini.tome"));
    }

    #[test]
    fn other_closes_show_what_was_said() {
        let e = classify(&node(), Some(1), "boom\n", "without responding");
        assert_eq!(e.kind, ErrorKind::Internal);
        assert_eq!(
            e.message,
            "the connection to mini closed without responding: boom"
        );
    }
}
