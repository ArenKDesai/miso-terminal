use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
        let path = self.path_for(url);
        let file = fs::File::open(&path).ok()?;
        let mut out = Vec::new();
        GzDecoder::new(file).read_to_end(&mut out).ok()?;
        // Mark as recently used so pruning evicts the least-recently-read files first.
        if let Ok(f) = fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
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
        self.files().iter().map(|f| f.1).sum()
    }

    /// Size on disk outside the `keep` directories: the part [`Self::prune`]
    /// governs.
    pub fn size_bytes_outside(&self, keep: &[PathBuf]) -> u64 {
        self.files()
            .iter()
            .filter(|f| !keep.iter().any(|k| f.0.starts_with(k)))
            .map(|f| f.1)
            .sum()
    }

    /// Every cached file with its size and last-use time.
    fn files(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        fn walk(p: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
            let Ok(rd) = fs::read_dir(p) else { return };
            for e in rd.flatten() {
                match e.metadata() {
                    Ok(m) if m.is_dir() => walk(&e.path(), out),
                    Ok(m) => out.push((
                        e.path(),
                        m.len(),
                        m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    )),
                    Err(_) => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.dir, &mut out);
        out
    }

    /// Delete least-recently-used files until the cache is at most `max_bytes`,
    /// leaving anything under the `keep` directories alone (and out of the
    /// total): data the app keeps on purpose has its own retention.
    /// Returns `(files removed, bytes freed)`.
    pub fn prune(&self, max_bytes: u64, keep: &[PathBuf]) -> (usize, u64) {
        let mut files = self.files();
        files.retain(|f| !keep.iter().any(|k| f.0.starts_with(k)));
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.2);
        let (mut removed, mut freed) = (0, 0);
        for (path, len, _) in files {
            if total <= max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
                freed += len;
            }
        }
        (removed, freed)
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

    #[test]
    fn prune_evicts_least_recently_used_first() {
        let dir = std::env::temp_dir().join(format!("mt-cache-prune-{}", std::process::id()));
        let cache = DiskCache::new(&dir);
        let (a, b, c) = ("https://h/a.csv", "https://h/b.csv", "https://h/c.csv");
        let body = vec![b'x'; 10_000];
        // The oldest file of all, but in a directory that is kept.
        let kept = "local://archive/day";
        cache.put(kept, &body).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        for url in [a, b, c] {
            cache.put(url, &body).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        cache.get(a).unwrap(); // a becomes the most recently used
        let per_file = cache.size_bytes() / 4;
        let archive = cache.path_for(kept).parent().unwrap().to_path_buf();
        let (removed, freed) = cache.prune(per_file * 2, std::slice::from_ref(&archive));
        assert_eq!(
            removed, 1,
            "the kept directory does not count towards the cap"
        );
        assert_eq!(
            cache.size_bytes_outside(std::slice::from_ref(&archive)),
            cache.size_bytes() - per_file
        );
        assert!(freed > 0);
        assert!(cache.get(b).is_none(), "b was least recently used");
        assert!(cache.get(a).is_some() && cache.get(c).is_some());
        assert!(cache.get(kept).is_some());
        cache.clear().unwrap();
    }
}
