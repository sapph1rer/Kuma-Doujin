//! Size-capped on-disk byte cache with LRU eviction (file mtime = last use) and optional
//! expiry of entries unused for longer than `max_age`. Used for covers and page images.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

pub struct DiskCache {
    dir: PathBuf,
    max_bytes: AtomicU64,
    max_age: Option<Duration>,
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn key_hash(key: &str) -> u64 {
    key.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

impl DiskCache {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self {
            dir,
            max_bytes: AtomicU64::new(max_bytes),
            max_age: None,
        }
    }

    /// Entries not used for longer than `age` are deleted on the next eviction pass.
    pub fn with_max_age(mut self, age: Duration) -> Self {
        self.max_age = Some(age);
        self
    }

    /// Changes the size cap; takes effect on the next eviction pass.
    pub fn set_max_bytes(&self, bytes: u64) {
        self.max_bytes.store(bytes, Ordering::Relaxed);
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{:016x}", key_hash(key)))
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let path = self.path(key);
        let bytes = fs::read(&path).ok()?;
        // Touch for LRU; failure only affects eviction order.
        let _ = fs::File::options()
            .append(true)
            .open(&path)
            .and_then(|f| f.set_modified(SystemTime::now()));
        Some(bytes)
    }

    pub fn put(&self, key: &str, bytes: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self.path(key).with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, self.path(key))?;
        self.evict()
    }

    /// Deletes expired files, then least-recently-used ones until the cache fits `max_bytes`.
    pub fn evict(&self) -> std::io::Result<()> {
        let max_bytes = self.max_bytes.load(Ordering::Relaxed);
        let now = SystemTime::now();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = fs::read_dir(&self.dir)?
            .filter_map(|e| {
                let e = e.ok()?;
                let m = e.metadata().ok()?;
                m.is_file().then_some(())?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.0);
        for (modified, len, path) in files {
            let expired = self
                .max_age
                .is_some_and(|age| now.duration_since(modified).unwrap_or_default() > age);
            if total <= max_bytes && !expired {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
        Ok(())
    }

    pub fn clear(&self) -> std::io::Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn roundtrip_evicts_lru_and_clears() {
        let dir = std::env::temp_dir().join(format!("kuma-disk-{}", std::process::id()));
        let cache = DiskCache::new(dir.clone(), 25);
        cache.put("a", &[1; 10]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        cache.put("b", &[2; 10]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get("a").unwrap(), vec![1; 10]); // a is now most recent
        std::thread::sleep(Duration::from_millis(20));
        cache.put("c", &[3; 10]).unwrap(); // 30 bytes > 25: evict LRU = b
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some() && cache.get("c").is_some());
        cache.clear().unwrap();
        assert!(cache.get("a").is_none());
        cache.clear().unwrap();
    }

    #[test]
    fn expires_unused_entries_and_honors_new_cap() {
        let dir = std::env::temp_dir().join(format!("kuma-disk-age-{}", std::process::id()));
        let cache = DiskCache::new(dir.clone(), 1_000).with_max_age(Duration::from_secs(3600));
        cache.put("old", &[1; 10]).unwrap();
        let old = SystemTime::now() - Duration::from_secs(7200);
        fs::File::options()
            .append(true)
            .open(cache.path("old"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        cache.put("new", &[2; 10]).unwrap();
        assert!(cache.get("old").is_none());
        assert!(cache.get("new").is_some());

        cache.set_max_bytes(5);
        cache.evict().unwrap();
        assert!(cache.get("new").is_none());
        cache.clear().unwrap();
    }
}
