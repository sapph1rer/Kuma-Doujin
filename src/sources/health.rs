//! Per-source health: success/failure counts, latency and last error, shown on the Sources page.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::core::errors::{SourceError, SourceResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unknown,
    Healthy,
    /// The latest call failed.
    Degraded,
    /// Several calls in a row failed.
    Down,
}

const DOWN_AFTER: u32 = 3;

#[derive(Debug, Clone, Default)]
pub struct Health {
    pub ok: u64,
    pub failed: u64,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    total_latency: Duration,
}

impl Health {
    pub fn status(&self) -> Status {
        match (self.ok + self.failed, self.consecutive_failures) {
            (0, _) => Status::Unknown,
            (_, n) if n >= DOWN_AFTER => Status::Down,
            (_, n) if n > 0 => Status::Degraded,
            _ => Status::Healthy,
        }
    }

    pub fn avg_latency(&self) -> Option<Duration> {
        (self.ok > 0).then(|| self.total_latency / self.ok as u32)
    }
}

#[derive(Default)]
pub struct HealthBook {
    sources: Mutex<HashMap<String, Health>>,
}

impl HealthBook {
    /// Records the outcome of a call that started at `started`. "Not found" is a valid
    /// answer from a working source, so it counts as success.
    pub fn record<T>(&self, source: &str, started: Instant, result: &SourceResult<T>) {
        let Ok(mut map) = self.sources.lock() else {
            return;
        };
        let h = map.entry(source.to_string()).or_default();
        match result {
            Ok(_) | Err(SourceError::NotFound) => {
                h.ok += 1;
                h.consecutive_failures = 0;
                h.total_latency += started.elapsed();
            }
            Err(e) => {
                h.failed += 1;
                h.consecutive_failures += 1;
                h.last_error = Some(e.to_string());
            }
        }
    }

    pub fn get(&self, source: &str) -> Health {
        self.sources
            .lock()
            .ok()
            .and_then(|m| m.get(source).cloned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_transitions() {
        let book = HealthBook::default();
        assert_eq!(book.get("s").status(), Status::Unknown);
        let t = Instant::now();
        book.record::<()>("s", t, &Ok(()));
        assert_eq!(book.get("s").status(), Status::Healthy);
        book.record::<()>("s", t, &Err(SourceError::NotFound));
        assert_eq!(book.get("s").status(), Status::Healthy);
        for _ in 0..2 {
            book.record::<()>("s", t, &Err(SourceError::Network("timeout".into())));
        }
        let h = book.get("s");
        assert_eq!(h.status(), Status::Degraded);
        assert_eq!(h.last_error.as_deref(), Some("network error: timeout"));
        book.record::<()>("s", t, &Err(SourceError::AccessDenied));
        assert_eq!(book.get("s").status(), Status::Down);
        book.record::<()>("s", t, &Ok(()));
        let h = book.get("s");
        assert_eq!(h.status(), Status::Healthy);
        assert_eq!((h.ok, h.failed), (3, 3));
        assert!(h.avg_latency().is_some());
    }
}
