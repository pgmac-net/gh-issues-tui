//! On-disk caches between the app and the backends (#226, ADR 0010).
//!
//! All of it is derived data that is safe to delete, so it lives in the user
//! cache directory, one directory per backend, host and org:
//!
//! ```text
//! ~/.cache/gh-issues/<provider>@<host>/<org>/
//!     snapshot.json   the last loaded issues and the scope they were fetched at
//!     readiness.json  Jev readiness answers, keyed by what was sent
//!     search.json     Jev search answers for the last few queries
//! ```
//!
//! The cache is **read-only**: writes go to the backend and the cache follows.
//! A missing, unreadable, corrupt or old-version file reads as empty, never as
//! an error; the worst outcome is fetching or asking again. Writes are atomic
//! and private (`0600` files in `0700` directories on Unix), because the
//! snapshot holds the text of private issues.

pub mod answers;
pub mod fsio;
pub mod snapshot;

use std::path::{Path, PathBuf};

/// `~/.cache/gh-issues` (platform equivalent elsewhere).
pub fn root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gh-issues")
}

/// Where one org's caches live, under `root`.
pub fn org_dir(root: &Path, provider: &str, host: &str, org: &str) -> PathBuf {
    root.join(format!("{}@{}", sanitise(provider), sanitise(host)))
        .join(sanitise(org))
}

/// `--no-cache`: forget everything cached for this org. The label-rank cache
/// lives beside these directories, not in them, so it is kept.
pub fn wipe(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Names come from flags, env and git remotes, so they are reduced to
/// characters safe in any file name.
fn sanitise(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.chars().all(|c| c == '.') {
        "_".into()
    } else {
        s
    }
}

/// FNV-1a, 64-bit: a hash that is stable across Rust releases and runs, so a
/// key written by one build is found by the next (`DefaultHasher` makes no
/// such promise). Not a security boundary: a collision only ever matters
/// between two versions of the *same* issue's request.
pub fn stable_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The hash of a Jev request body: everything that was sent, model and
/// question text included, so changing any of them is a miss.
pub fn request_hash(body: &serde_json::Value) -> u64 {
    stable_hash(body.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_org_on_each_host_has_its_own_directory() {
        let root = Path::new("/c");
        assert_eq!(
            org_dir(root, "github", "github.com", "pgmac-net"),
            Path::new("/c/github@github.com/pgmac-net")
        );
        assert_ne!(
            org_dir(root, "jira", "a.atlassian.net", "OPS"),
            org_dir(root, "jira", "b.atlassian.net", "OPS")
        );
        assert_eq!(
            org_dir(root, "github", "github.com", "../x"),
            Path::new("/c/github@github.com/.._x")
        );
        assert_eq!(
            org_dir(root, "github", "", ".."),
            Path::new("/c/github@_/_")
        );
    }

    #[test]
    fn the_hash_is_fnv1a_and_so_stable_across_builds() {
        assert_eq!(stable_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn any_change_to_a_request_changes_its_hash() {
        let a = serde_json::json!({"state": {"title": "t"}, "model": "m1"});
        let b = serde_json::json!({"state": {"title": "t"}, "model": "m2"});
        assert_eq!(request_hash(&a), request_hash(&a.clone()));
        assert_ne!(request_hash(&a), request_hash(&b));
    }

    #[test]
    fn wiping_a_missing_directory_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nope");
        wipe(&dir).unwrap();
        std::fs::create_dir_all(dir.join("x")).unwrap();
        wipe(&dir).unwrap();
        assert!(!dir.exists());
    }
}
