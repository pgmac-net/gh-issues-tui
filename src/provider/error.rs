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

    /// The thing asked for does not exist, or the credentials can no longer
    /// see it (GitHub `NOT_FOUND`, Jira 404, Linear "Entity not found").
    /// Displays like the `Api` error it used to be.
    #[error("API errors: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, ProviderError>;

/// Turn a [`ProviderError::NotFound`] into `Ok(None)`: for the single-item
/// reads behind a refresh (#229), "gone" is an answer, not a failure.
pub fn found<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(ProviderError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_maps_only_not_found_to_none() {
        assert_eq!(found(Ok(1)).unwrap(), Some(1));
        assert_eq!(
            found::<u8>(Err(ProviderError::NotFound("gone".into()))).unwrap(),
            None
        );
        assert!(found::<u8>(Err(ProviderError::Api("boom".into()))).is_err());
    }

    #[test]
    fn not_found_displays_like_an_api_error() {
        assert_eq!(
            ProviderError::NotFound("nope".into()).to_string(),
            ProviderError::Api("nope".into()).to_string()
        );
    }
}
