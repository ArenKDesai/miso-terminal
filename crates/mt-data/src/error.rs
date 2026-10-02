/// Why a fetch failed. `Clone` so the hub can keep the last error next to the
/// last good value.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum FetchError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
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
