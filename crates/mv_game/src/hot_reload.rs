//! Hot reload of the material and reaction RON files (debug builds, or
//! `--hot-reload`).
//!
//! Polls the modification time and size of every `*.ron` file in the watched
//! directories a couple of times per second; no file-watching dependency needed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

const POLL_PERIOD: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub struct HotReload {
    dirs: Vec<PathBuf>,
    fingerprint: u64,
    since_poll: Duration,
}

impl HotReload {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        let fingerprint = fingerprint(&dirs);
        Self {
            dirs,
            fingerprint,
            since_poll: Duration::ZERO,
        }
    }

    /// Returns `true` once after any watched file is added, removed or modified.
    pub fn poll(&mut self, dt: Duration) -> bool {
        self.since_poll += dt;
        if self.since_poll < POLL_PERIOD {
            return false;
        }
        self.since_poll = Duration::ZERO;
        let now = fingerprint(&self.dirs);
        let changed = now != self.fingerprint;
        self.fingerprint = now;
        changed
    }
}

/// Hash of (path, modification time, size) of every `*.ron` file in `dirs`.
fn fingerprint(dirs: &[PathBuf]) -> u64 {
    let mut entries: Vec<(PathBuf, Option<std::time::SystemTime>, u64)> = dirs
        .iter()
        .flat_map(|dir| ron_files(dir))
        .map(|path| {
            let meta = std::fs::metadata(&path).ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok());
            let len = meta.map_or(0, |m| m.len());
            (path, modified, len)
        })
        .collect();
    entries.sort();
    let mut h = DefaultHasher::new();
    entries.hash(&mut h);
    h.finish()
}

fn ron_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|ext| ext == "ron"))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_changes() {
        let dir = std::env::temp_dir().join(format!("chemxel_hot_reload_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ron");
        std::fs::write(&file, "[]").unwrap();
        let mut hr = HotReload::new(vec![dir.clone()]);
        assert!(!hr.poll(POLL_PERIOD), "nothing changed yet");
        std::fs::write(&file, "[ ]").unwrap(); // size changes even if mtime does not
        assert!(!hr.poll(Duration::ZERO), "polls are rate limited");
        assert!(hr.poll(POLL_PERIOD));
        assert!(!hr.poll(POLL_PERIOD), "reported once");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
