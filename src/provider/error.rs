use thiserror::Error;

/// Stable prefix for rate-limit error messages. The TUI event handler
/// classifies stringified task errors by this prefix — keep them in sync
/// via this constant, never a literal.
pub const RATE_LIMIT_MSG_PREFIX: &str = "API rate limit exceeded";

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API errors: {0}")]
    Api(String),

    #[error("unexpected response shape: {0}")]
    Shape(String),

    #[error("{0}")]
    RateLimited(String),

    #[error("query too large for the provider's limits even at the smallest page size ({0})")]
    ResourceLimited(String),

    #[error("not supported by this provider: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, ProviderError>;
