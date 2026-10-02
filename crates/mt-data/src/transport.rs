use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;

use crate::FetchError;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where bytes come from. Queries never see this directly; they go through
/// [`crate::FetchCtx`], which adds caching, throttling and logging on top.
pub trait Transport: Send + Sync + 'static {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Bytes, FetchError>>;

    /// Short description for the status bar.
    fn describe(&self) -> String;

    /// Whether this transport reaches the real source (false for replays).
    fn is_live(&self) -> bool {
        true
    }
}

/// The real network.
pub struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new(user_agent: &str) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(10))
            // The rolling five-minute feed is ~7 MB gzipped; give it room.
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| FetchError::Other(format!("could not build HTTP client: {e}")))?;
        Ok(Self { client })
    }
}

impl Transport for HttpTransport {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Bytes, FetchError>> {
        Box::pin(async move {
            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| FetchError::Network(e.to_string()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(FetchError::NotFound(url.to_owned()));
            }
            if !status.is_success() {
                return Err(FetchError::Status {
                    status: status.as_u16(),
                    url: url.to_owned(),
                });
            }
            resp.bytes()
                .await
                .map_err(|e| FetchError::Network(e.to_string()))
        })
    }

    fn describe(&self) -> String {
        "MISO public data".into()
    }
}

/// Serves recorded responses from a directory, for offline work and tests.
///
/// `https://host/a/b` maps to `<root>/host/a/b`, with `.json` appended when the
/// URL path has no extension. Date-stamped files (`20261001_da_expost_lmp.csv`)
/// fall back to the newest fixture with the same suffix, so any date "replays"
/// the recorded day.
pub struct FixtureTransport {
    root: PathBuf,
}

impl FixtureTransport {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, url: &str) -> PathBuf {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let mut path = self.root.clone();
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            path.push(part);
        }
        if path.extension().is_none() {
            path.set_extension("json");
        }
        path
    }

    fn dated_fallback(path: &Path) -> Option<PathBuf> {
        let name = path.file_name()?.to_str()?;
        let (date, suffix) = name.split_once('_')?;
        if date.len() != 8 || !date.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(path.parent()?)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.split_once('_'))
                    .is_some_and(|(d, s)| d.len() == 8 && s == suffix)
            })
            .collect();
        candidates.sort();
        candidates.pop()
    }
}

impl Transport for FixtureTransport {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Bytes, FetchError>> {
        Box::pin(async move {
            let path = self.path_for(url);
            let path = if path.exists() {
                path
            } else {
                Self::dated_fallback(&path).ok_or_else(|| FetchError::NotFound(url.to_owned()))?
            };
            std::fs::read(&path)
                .map(Bytes::from)
                .map_err(|e| FetchError::Other(format!("{}: {e}", path.display())))
        })
    }

    fn describe(&self) -> String {
        format!("replaying {}", self.root.display())
    }

    fn is_live(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_paths() {
        let t = FixtureTransport::new("/fx");
        assert_eq!(
            t.path_for("https://public-api.misoenergy.org/api/FuelMix"),
            Path::new("/fx/public-api.misoenergy.org/api/FuelMix.json")
        );
        assert_eq!(
            t.path_for("https://docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv"),
            Path::new("/fx/docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv")
        );
    }

    #[tokio::test]
    async fn dated_files_fall_back_to_newest_recording() {
        let root = std::env::temp_dir().join(format!("mt-fixture-test-{}", std::process::id()));
        let dir = root.join("host").join("reports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("20260101_da.csv"), "old").unwrap();
        std::fs::write(dir.join("20260301_da.csv"), "new").unwrap();
        std::fs::write(dir.join("20260401_rt.csv"), "other").unwrap();
        let t = FixtureTransport::new(&root);
        let got = t.get("https://host/reports/20991231_da.csv").await.unwrap();
        assert_eq!(got.as_ref(), b"new");
        assert!(matches!(
            t.get("https://host/reports/missing.csv").await,
            Err(FetchError::NotFound(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
