use std::time::Duration;
use thiserror::Error;

/// Errors a provider may return. The retry policy keys off [`SourceError::is_transient`].
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("network error: {0}")]
    Network(String),
    #[error("rate limited by source")]
    RateLimited { retry_after: Option<Duration> },
    /// 401/403 or a policy refusal. Never retried.
    #[error("access denied by source")]
    AccessDenied,
    #[error("not found")]
    NotFound,
    #[error("invalid response from source: {0}")]
    InvalidResponse(String),
    /// The source does not offer this operation (e.g. metadata-only sources and page images).
    #[error("operation not supported by this source")]
    Unsupported,
}

impl SourceError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Network(_) | Self::RateLimited { .. })
    }
}

pub type SourceResult<T> = Result<T, SourceError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_network_and_rate_limit_are_retryable() {
        assert!(SourceError::Network("timeout".into()).is_transient());
        assert!(SourceError::RateLimited { retry_after: None }.is_transient());
        assert!(!SourceError::AccessDenied.is_transient());
        assert!(!SourceError::NotFound.is_transient());
        assert!(!SourceError::Unsupported.is_transient());
    }
}
