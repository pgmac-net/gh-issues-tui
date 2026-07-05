use thiserror::Error;

/// Stable prefix for rate-limit error messages. The TUI event handler
/// classifies stringified task errors by this prefix — keep them in sync
/// via this constant, never a literal.
pub const RATE_LIMIT_MSG_PREFIX: &str = "API rate limit exceeded";

#[derive(Debug, Error)]
pub enum GithubError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("GraphQL errors: {0}")]
    GraphQl(String),

    #[error("unexpected response shape: {0}")]
    Shape(String),

    #[error("{0}")]
    RateLimited(String),
}

pub type Result<T> = std::result::Result<T, GithubError>;
