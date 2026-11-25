//! Local capture history: one JSON object per line in `history.jsonl`.
//! Nothing here ever leaves the machine.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    /// Unix seconds.
    pub time: u64,
    pub width: u32,
    pub height: u32,
    pub kind: String,
}

impl Entry {
    pub fn new(path: PathBuf, width: u32, height: u32, kind: &str) -> Entry {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Entry { path, time, width, height, kind: kind.into() }
    }
}

pub fn append(file: &std::path::Path, e: &Entry) -> std::io::Result<()> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
    let line = serde_json::to_string(e).map_err(std::io::Error::other)?;
    writeln!(f, "{line}")
}

/// Newest first, skipping corrupt lines, at most `limit` entries.
pub fn load(file: &std::path::Path, limit: usize) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let mut v: Vec<Entry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v.truncate(limit);
    v
}

/// Rewrites the file keeping only the newest `limit` entries (oldest first on disk).
pub fn rewrite(file: &std::path::Path, newest_first: &[Entry]) -> std::io::Result<()> {
    let mut out = String::new();
    for e in newest_first.iter().rev() {
        out.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    std::fs::write(file, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_load_and_trim() {
        let f = std::env::temp_dir().join(format!("snapit-hist-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&f);
        for i in 0..5 {
            append(&f, &Entry::new(format!("{i}.png").into(), 1, 1, "region")).unwrap();
        }
        std::fs::OpenOptions::new().append(true).open(&f).unwrap().write_all(b"garbage\n").unwrap();
        let v = load(&f, 3);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].path, PathBuf::from("4.png"));
        rewrite(&f, &v).unwrap();
        assert_eq!(load(&f, 100).len(), 3);
        std::fs::remove_file(&f).unwrap();
    }
}
