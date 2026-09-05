//! Fixed-window rate limiting for the public `/auth/*` routes
//! (`multi-user.md`: "remaining, roughly in priority for a public host:
//! rate-limiting... Argon2id's ~10-40 ms per attempt is friction, not a
//! defence against a botnet").
//!
//! One process, in-memory, keyed by an arbitrary caller-chosen string (a
//! source IP, a target nickname, prefixed per route so the same map serves
//! every caller) — good enough for a single-instance host; a
//! multi-instance deployment would need a shared store instead, same
//! caveat as `MuWeb`'s session table.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The limit under `key` was exceeded; retry no sooner than this.
#[derive(Debug, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after_secs: u64,
}

struct Window {
    started: Instant,
    count: u32,
}

#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<String, Window>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one attempt under `key`. Ok while the count inside the
    /// current `window` is at or below `limit`; once it is exceeded every
    /// further call errors until the window rolls over.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> Result<(), RateLimited> {
        let now = Instant::now();
        let mut guard = self.windows.lock().unwrap();
        let w = guard.entry(key.to_string()).or_insert_with(|| Window {
            started: now,
            count: 0,
        });
        if now.duration_since(w.started) >= window {
            w.started = now;
            w.count = 0;
        }
        w.count += 1;
        if w.count > limit {
            let retry = window.saturating_sub(now.duration_since(w.started));
            Err(RateLimited {
                retry_after_secs: retry.as_secs().max(1),
            })
        } else {
            Ok(())
        }
    }

    /// Drop windows that closed at least `max_age` ago, so one-shot callers
    /// (a single request from an IP never seen again) don't accumulate
    /// forever. Call periodically, same shape as `MuWeb`'s session sweep.
    pub fn sweep(&self, max_age: Duration) {
        let now = Instant::now();
        self.windows
            .lock()
            .unwrap()
            .retain(|_, w| now.duration_since(w.started) <= max_age);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_rejects() {
        let rl = RateLimiter::new();
        let window = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(rl.check("k", 3, window).is_ok());
        }
        let err = rl.check("k", 3, window).unwrap_err();
        assert!(err.retry_after_secs >= 1 && err.retry_after_secs <= 60);
    }

    #[test]
    fn independent_keys_do_not_share_a_budget() {
        let rl = RateLimiter::new();
        let window = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(rl.check("a", 3, window).is_ok());
        }
        assert!(rl.check("a", 3, window).is_err());
        // a fresh key still has its own full budget
        assert!(rl.check("b", 3, window).is_ok());
    }

    #[test]
    fn a_window_rolls_over() {
        let rl = RateLimiter::new();
        // a window so short it is already elapsed by the second call
        let window = Duration::from_millis(1);
        assert!(rl.check("k", 1, window).is_ok());
        std::thread::sleep(Duration::from_millis(5));
        assert!(rl.check("k", 1, window).is_ok(), "new window, budget reset");
    }

    #[test]
    fn sweep_drops_only_windows_older_than_max_age() {
        let rl = RateLimiter::new();
        rl.check("stale", 5, Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        rl.check("fresh", 5, Duration::from_secs(60)).unwrap();

        rl.sweep(Duration::from_millis(5));

        assert!(rl.windows.lock().unwrap().contains_key("fresh"));
        assert!(!rl.windows.lock().unwrap().contains_key("stale"));
    }
}
