//! The daemon's shutdown signal: set once, seen by every background loop,
//! and waking any of them that are waiting between ticks.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Stop {
    stopping: Mutex<bool>,
    woken: Condvar,
}

impl Stop {
    /// Start shutting down, waking every waiter.
    pub fn set(&self) {
        *self.stopping.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.woken.notify_all();
    }

    pub fn is_set(&self) -> bool {
        *self.stopping.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait out `d`, or until shutdown starts. True if the wait ran its
    /// course; false once shutting down.
    pub fn sleep(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        let mut stopping = self.stopping.lock().unwrap_or_else(|p| p.into_inner());
        while !*stopping {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return true;
            };
            stopping = self
                .woken
                .wait_timeout(stopping, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_sleep_runs_its_course_until_stopped() {
        let stop = Stop::default();
        assert!(stop.sleep(Duration::from_millis(1)));
        stop.set();
        assert!(stop.is_set());
        assert!(
            !stop.sleep(Duration::from_secs(60)),
            "no waiting once stopped"
        );
    }

    #[test]
    fn stopping_wakes_a_sleeper() {
        let stop = Arc::new(Stop::default());
        let sleeper = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || stop.sleep(Duration::from_secs(60)))
        };
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        stop.set();
        assert!(!sleeper.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
