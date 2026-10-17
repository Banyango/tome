//! Daemon notifications that aren't tied to a run's pane: logged, and sent to
//! cmux unless `TOME_NOTIFY=off`.

use super::Cmux;

/// A trigger failed to act (`what` names the workflow or project). It's
/// always logged; it also goes out as a cmux notification unless
/// `TOME_NOTIFY=off` (there's no run pane to tie it to, so this is best
/// effort when cmux isn't around).
pub fn notify_trigger(what: &str, message: &str) {
    eprintln!("tome daemon: notify: trigger for {what} failed: {message}");
    if std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
        Cmux.notify(&format!("tome: trigger for {what} failed"), message);
    }
}

/// A run that claimed an event didn't succeed, so its delivery failed.
/// Logged, and sent as a cmux notification unless `TOME_NOTIFY=off`.
pub fn notify_delivery(title: &str, message: &str) {
    eprintln!("tome daemon: notify: {title}: {message}");
    if std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
        Cmux.notify(&format!("tome: {title}"), message);
    }
}
