//! Remote workers (`tome worker spawn --node`): a worker whose session runs
//! on a node while the run, and the worker's record, stay with this daemon.
//!
//! - [`link`] is the run's side: one SSH link per node, which spawns and
//!   closes hosted workers, polls for their calls and ends, and answers the
//!   calls here.
//! - [`host`] is the node's side: it runs the sessions, keeps a small
//!   `hosted_workers` record of each, and queues its workers' tome calls
//!   for the link to pick up.

pub mod host;
pub mod link;
pub mod spawn;

use std::time::Duration;

/// The node RPCs a run's daemon calls over a link.
pub const METHODS: &[&str] = &["hosted.spawn", "hosted.close", "hosted.attach"];

/// What `daemon.ping` lists for a node that can host workers.
pub const CAPABILITY: &str = "hosted_workers";

/// Set in a hosted worker's session: the daemon its calls go up to.
pub const HOSTED_ENV: &str = "TOME_HOSTED";

/// A remote worker whose node stopped answering.
pub const NODE_UNREACHABLE: &str = "node_unreachable";

/// How long a hosted worker's call waits for its reply.
const RELAY_TIMEOUT: Duration = Duration::from_secs(300);

/// A node that hasn't been polled for this long has lost its upstream.
const LOST_AFTER: Duration = Duration::from_secs(60);

/// A short, stable tag for an origin, for names and paths.
fn tag(origin: &str) -> String {
    // FNV-1a: stable across builds, unlike `DefaultHasher`.
    let hash = origin.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    format!("{:08x}", hash as u32)
}

/// A hosted worker's session name. Unlike a run's own sessions
/// (`tome-<run>-...`), it can't be caught by a node run's cleanup.
fn session_name(origin: &str, run_id: i64, name: &str) -> String {
    format!("tome-hosted-{}-{run_id}-{name}", tag(origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_stable_and_distinct() {
        assert_eq!(tag("a"), tag("a"));
        assert_ne!(tag("a"), tag("b"));
        assert_eq!(tag("a").len(), 8);
        assert!(session_name("a", 3, "w").starts_with("tome-hosted-"));
    }
}
