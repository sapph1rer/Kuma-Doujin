use std::future::Future;
use std::time::Duration;

use crate::core::errors::{SourceError, SourceResult};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
        }
    }
}

impl RetryPolicy {
    /// Exponential backoff (`base * 2^attempt`), or the server's hint, capped at `max_delay`.
    fn delay(&self, attempt: u32, err: &SourceError) -> Duration {
        let hinted = match err {
            SourceError::RateLimited { retry_after } => *retry_after,
            _ => None,
        };
        hinted
            .unwrap_or(self.base_delay.saturating_mul(1 << attempt.min(16)))
            .min(self.max_delay)
    }
}

/// Runs `op` until it succeeds, fails permanently, or attempts run out.
/// Only [`SourceError::is_transient`] errors are retried; access-denied never is.
pub async fn with_retry<T, F, Fut>(policy: RetryPolicy, mut op: F) -> SourceResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = SourceResult<T>>,
{
    let mut attempt = 0;
    loop {
        match op().await {
            Err(e) if e.is_transient() && attempt + 1 < policy.max_attempts => {
                let wait = policy.delay(attempt, &e);
                tracing::warn!(error = %e, attempt, ?wait, "retrying request");
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const FAST: RetryPolicy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    };

    #[tokio::test]
    async fn retries_transient_then_succeeds() {
        let calls = Cell::new(0);
        let res = with_retry(FAST, || {
            calls.set(calls.get() + 1);
            let n = calls.get();
            async move {
                if n < 3 {
                    Err(SourceError::Network("reset".into()))
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(res.unwrap(), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts_and_never_retries_denied() {
        let calls = Cell::new(0);
        let res: SourceResult<()> = with_retry(FAST, || {
            calls.set(calls.get() + 1);
            async { Err(SourceError::Network("down".into())) }
        })
        .await;
        assert!(res.is_err());
        assert_eq!(calls.get(), 3);

        calls.set(0);
        let res: SourceResult<()> = with_retry(FAST, || {
            calls.set(calls.get() + 1);
            async { Err(SourceError::AccessDenied) }
        })
        .await;
        assert!(matches!(res, Err(SourceError::AccessDenied)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn backoff_doubles_and_respects_hint_and_cap() {
        let p = RetryPolicy::default();
        let net = SourceError::Network(String::new());
        assert_eq!(p.delay(0, &net), Duration::from_millis(500));
        assert_eq!(p.delay(2, &net), Duration::from_secs(2));
        assert_eq!(p.delay(30, &net), Duration::from_secs(10));
        let rl = SourceError::RateLimited {
            retry_after: Some(Duration::from_secs(3)),
        };
        assert_eq!(p.delay(0, &rl), Duration::from_secs(3));
    }
}
