use thiserror::Error;

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
