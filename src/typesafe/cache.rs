//! On-disk cache of inferred label ranks.
//!
//! Lives in the user cache directory, not next to `config.toml`: the config
//! is hand-authored, this is derived data that is safe to delete.
//!
//! Keyed per `(org, label)` rather than by label alone, because a label can
//! mean different things in different organisations (`blocked` may be
//! "escalate now" in one and "parked" in another) and a global key would let
//! whichever org was seen first decide that for all of them, silently.
//!
//! There is no TTL — a label's meaning does not drift on a timescale worth
//! expiring. An entry is a miss when it was produced by a different model,
//! and the whole file is discarded when the prompt version differs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MODEL, PROMPT_VERSION};

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    /// `None` is a real, cached answer: "this label is not about priority".
    rank: Option<u8>,
    model: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct File {
    prompt_version: u32,
    entries: HashMap<String, Entry>,
}

impl Default for File {
    fn default() -> Self {
        Self {
            prompt_version: PROMPT_VERSION,
            entries: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct RankCache {
    path: PathBuf,
    file: File,
}

/// `~/.cache/gh-issues/label-ranks.json` (platform equivalent elsewhere).
pub fn default_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gh-issues")
        .join("label-ranks.json")
}

/// NUL cannot appear in an org or label name, so the pair cannot collide.
fn key(org: &str, label: &str) -> String {
    format!("{org}\0{label}")
}

impl RankCache {
    /// A missing, unreadable, corrupt or stale-version file is an empty cache,
    /// never an error: the worst outcome is asking again.
    pub fn load(path: &Path) -> Self {
        let file = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<File>(&raw).ok())
            .filter(|f| f.prompt_version == PROMPT_VERSION)
            .unwrap_or_default();
        Self {
            path: path.to_path_buf(),
            file,
        }
    }

    /// `Some(rank)` on a hit (where `rank` may itself be `None`), `None` on a
    /// miss.
    pub fn get(&self, org: &str, label: &str) -> Option<Option<u8>> {
        self.file
            .entries
            .get(&key(org, label))
            .filter(|e| e.model == MODEL)
            .map(|e| e.rank)
    }

    pub fn insert(&mut self, org: &str, label: &str, rank: Option<u8>) {
        self.file.entries.insert(
            key(org, label),
            Entry {
                rank,
                model: MODEL.to_string(),
            },
        );
    }

    /// Written to a sibling temp file then renamed, so a crash mid-write
    /// cannot leave a truncated cache behind.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&self.file)?)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested").join("label-ranks.json");
        (dir, p)
    }

    #[test]
    fn a_saved_rank_round_trips_including_a_cached_none() {
        let (_dir, p) = path();
        let mut c = RankCache::load(&p);
        c.insert("acme", "P0", Some(4));
        c.insert("acme", "chore", None);
        c.save().unwrap();

        let c = RankCache::load(&p);
        assert_eq!(c.get("acme", "P0"), Some(Some(4)));
        // A cached "not a priority label" is a hit, not a miss.
        assert_eq!(c.get("acme", "chore"), Some(None));
        assert_eq!(c.get("acme", "unseen"), None);
    }

    #[test]
    fn the_same_label_in_another_org_is_a_miss() {
        let (_dir, p) = path();
        let mut c = RankCache::load(&p);
        c.insert("acme", "blocked", Some(4));
        assert_eq!(c.get("other", "blocked"), None);
    }

    #[test]
    fn an_entry_from_a_different_model_is_a_miss() {
        let (_dir, p) = path();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let stale = serde_json::json!({
            "prompt_version": PROMPT_VERSION,
            "entries": { "acme\0P0": { "rank": 4, "model": "jev-0.0.1" } },
        });
        std::fs::write(&p, stale.to_string()).unwrap();
        assert_eq!(RankCache::load(&p).get("acme", "P0"), None);
    }

    #[test]
    fn a_different_prompt_version_discards_the_whole_file() {
        let (_dir, p) = path();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let old = serde_json::json!({
            "prompt_version": PROMPT_VERSION + 1,
            "entries": { "acme\0P0": { "rank": 4, "model": MODEL } },
        });
        std::fs::write(&p, old.to_string()).unwrap();
        assert_eq!(RankCache::load(&p).get("acme", "P0"), None);
    }

    #[test]
    fn a_corrupt_or_missing_file_is_an_empty_cache_not_an_error() {
        let (_dir, p) = path();
        assert_eq!(RankCache::load(&p).get("acme", "P0"), None);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(RankCache::load(&p).get("acme", "P0"), None);
    }
}
