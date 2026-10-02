use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

/// Gzipped on-disk cache for responses that never change once published
/// (settled market reports). File names mirror the URL so the cache is easy to
/// inspect or prune by hand.
#[derive(Clone, Debug)]
pub struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `https://host/a/b.csv?x=1` -> `<dir>/host/a/b.csv_x_1.gz`.
    pub fn path_for(&self, url: &str) -> PathBuf {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let mut path = self.dir.clone();
        let mut parts = rest.split('/').filter(|p| !p.is_empty()).peekable();
        while let Some(part) = parts.next() {
            let clean: String = part
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            if parts.peek().is_some() {
                path.push(clean);
            } else {
                path.push(format!("{clean}.gz"));
            }
        }
        path
    }

    pub fn get(&self, url: &str) -> Option<Bytes> {
        let file = fs::File::open(self.path_for(url)).ok()?;
        let mut out = Vec::new();
        GzDecoder::new(file).read_to_end(&mut out).ok()?;
        Some(Bytes::from(out))
    }

    /// Write atomically (temp file + rename) so a crash never leaves a torn entry.
    pub fn put(&self, url: &str, body: &[u8]) -> io::Result<()> {
        let path = self.path_for(url);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("gz.tmp");
        {
            let mut enc = GzEncoder::new(fs::File::create(&tmp)?, Compression::default());
            enc.write_all(body)?;
            enc.finish()?;
        }
        fs::rename(tmp, path)
    }

    /// Total size on disk, for the settings panel.
    pub fn size_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            fs::read_dir(p).map_or(0, |rd| {
                rd.flatten()
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => walk(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
        }
        walk(&self.dir)
    }

    pub fn clear(&self) -> io::Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_maps_urls_to_paths() {
        let dir = std::env::temp_dir().join(format!("mt-cache-test-{}", std::process::id()));
        let cache = DiskCache::new(&dir);
        let url = "https://docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv";
        assert!(cache.get(url).is_none());
        cache.put(url, b"hello,world").unwrap();
        assert_eq!(cache.get(url).unwrap().as_ref(), b"hello,world");
        assert!(
            cache
                .path_for(url)
                .ends_with("docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv.gz")
        );
        assert!(cache.size_bytes() > 0);
        cache.clear().unwrap();
        assert!(cache.get(url).is_none());
    }
}
