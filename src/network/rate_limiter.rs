use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Per-host minimum spacing between requests. Hosts without a configured
/// interval are not throttled (the client's concurrency cap still applies).
pub struct RateLimiter {
    intervals: HashMap<String, Duration>,
    next_slot: Mutex<HashMap<String, Instant>>,
}

impl RateLimiter {
    pub fn new(intervals: &[(&str, Duration)]) -> Self {
        Self {
            intervals: intervals.iter().map(|(h, d)| (h.to_string(), *d)).collect(),
            next_slot: Mutex::default(),
        }
    }

    /// Waits until this host may be called again, reserving the slot first so
    /// concurrent callers queue up instead of bursting.
    pub async fn acquire(&self, host: &str) {
        let Some(&interval) = self.intervals.get(host) else {
            return;
        };
        let slot = {
            let mut next = self.next_slot.lock().await;
            let now = Instant::now();
            let slot = next.get(host).copied().unwrap_or(now).max(now);
            next.insert(host.to_string(), slot + interval);
            slot
        };
        tokio::time::sleep_until(slot).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spaces_requests_per_host() {
        let rl = RateLimiter::new(&[("a.test", Duration::from_millis(30))]);
        let start = Instant::now();
        for _ in 0..3 {
            rl.acquire("a.test").await;
        }
        assert!(start.elapsed() >= Duration::from_millis(60));

        let start = Instant::now();
        for _ in 0..3 {
            rl.acquire("unthrottled.test").await;
        }
        assert!(start.elapsed() < Duration::from_millis(30));
    }
}
