/// Why a fetch failed. `Clone` so the hub can keep the last error next to the
/// last good value.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum FetchError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
    /// Missing, wrong or insufficient credentials (HTTP 401/403, a refused
    /// stream login, a key that is not set). Retrying will not help until the
    /// keys change, so streams stop reconnecting.
    #[error("not authorised: {0}")]
    Auth(String),
    /// HTTP 429. The request budget for the host stays closed for `retry_after_secs`.
    #[error("rate limited by {host} (retry in {retry_after_secs} s)")]
    RateLimited { host: String, retry_after_secs: u64 },
    #[error("network error: {0}")]
    Network(String),
    #[error("could not parse {what}: {detail}")]
    Parse { what: String, detail: String },
    #[error("{0}")]
    Other(String),
}

impl FetchError {
    pub fn parse(what: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        Self::Parse {
            what: what.into(),
            detail: detail.to_string(),
        }
    }
}

impl From<crate::SecretError> for FetchError {
    fn from(e: crate::SecretError) -> Self {
        Self::Other(e.to_string())
    }
}
