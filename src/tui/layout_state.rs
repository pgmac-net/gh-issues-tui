//! The saved layout (#236): where you were, so the next start resumes there.
//!
//! One JSON file per *view*, keyed by the org browsed and the clone the app
//! was started in, so the whole-org view and each clone keep their own place
//! and copies on different views never touch each other. Two copies on the
//! same view: the last to write wins, and every write is atomic.
//!
//! Not `tui::layout`, which is screen geometry. This is a file format; the
//! `App` side (taking a snapshot, applying one) is `app/resume.rs`.
//!
//! A missing, corrupt, wrong-version or other-provider file is ignored, never
//! an error: the worst outcome is starting fresh.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Bump when a field's meaning changes. An older file is then ignored.
pub const VERSION: u32 = 1;

/// What was selected: an issue by id, or a repo's header by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Selected {
    Issue(String),
    Repo(String),
}

/// The filters worth resuming. Semantic-search hits are session state and are
/// not saved: the text filter re-asks once data lands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedFilters {
    pub text: String,
    pub repo: String,
    pub assignee: String,
    pub author: String,
    pub priority: Vec<String>,
    pub status: Vec<String>,
    pub created_after: Option<NaiveDate>,
    pub created_before: Option<NaiveDate>,
    pub updated_after: Option<NaiveDate>,
    pub updated_before: Option<NaiveDate>,
    pub closed_after: Option<NaiveDate>,
    pub closed_before: Option<NaiveDate>,
    pub hide_empty: bool,
}

/// Everything a resumed session needs to look like the one that was left.
/// Lists are sorted, so equal states serialise equally and a change is a
/// real change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub version: u32,
    /// Which backend wrote it: issue ids differ between backends, so a layout
    /// from another one is ignored.
    pub provider: String,
    pub filters: SavedFilters,
    /// `open`, `closed` or `all`.
    pub state_filter: String,
    /// A sort key's label (`updated`, `priority`, …).
    pub sort_key: String,
    pub sort_desc: bool,
    /// Collapsed repo groups, by name.
    pub collapsed: Vec<String>,
    /// Repos already seen, so a repo you expanded stays expanded rather than
    /// being folded again as "new" by `default_collapsed`.
    pub seen_repos: Vec<String>,
    /// Collapsed sub-issue parents, by issue id (#234).
    pub collapsed_issues: Vec<String>,
    /// Folded comments, by comment id (#210).
    pub folded_comments: Vec<String>,
    pub selected: Option<Selected>,
    /// First visible list row.
    pub list_top: usize,
    pub detail_open: bool,
    pub detail_focused: bool,
    pub runs_tab: bool,
    /// The repo workflows view's grouping label (#266). Empty in a layout
    /// written before it existed, which reads as the default.
    #[serde(default)]
    pub runs_group: String,
}

/// `<state dir>/gh-issues/layout`. XDG state on Linux (`~/.local/state`,
/// honouring `$XDG_STATE_HOME`); macOS and Windows have no state directory, so
/// their local-data directory stands in.
pub fn dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gh-issues")
        .join("layout")
}

/// The file for one view in `dir`: `<org>.json`, or `<org>@<repo>.json` for a
/// view started inside a clone.
pub fn path_in(dir: &Path, org: &str, clone: Option<&str>) -> PathBuf {
    let stem = match clone {
        Some(repo) => format!("{}@{}", sanitise(org), sanitise(repo)),
        None => sanitise(org),
    };
    dir.join(format!("{stem}.json"))
}

/// Org and repo names come from a git remote or a flag, so they are reduced to
/// characters that are safe in any file name.
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
    // Never an empty name or a bare `.`/`..` component.
    if s.is_empty() || s.chars().all(|c| c == '.') {
        "_".into()
    } else {
        s
    }
}

/// The saved layout, if there is a usable one for `provider`.
pub fn load(path: &Path, provider: &str) -> Option<Layout> {
    let raw = std::fs::read_to_string(path).ok()?;
    let layout: Layout = serde_json::from_str(&raw).ok()?;
    (layout.version == VERSION && layout.provider == provider).then_some(layout)
}

/// Written to a per-process temp file in the same directory, then renamed, so
/// a crash mid-write cannot leave a truncated file and two copies saving the
/// same view cannot trip over one temp file.
pub fn save(path: &Path, layout: &Layout) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(layout)?)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Layout {
        Layout {
            version: VERSION,
            provider: "github".into(),
            filters: SavedFilters {
                text: "crash".into(),
                assignee: "pgmac".into(),
                priority: vec!["high".into()],
                updated_after: NaiveDate::from_ymd_opt(2026, 10, 1),
                hide_empty: true,
                ..Default::default()
            },
            state_filter: "all".into(),
            sort_key: "priority".into(),
            sort_desc: false,
            collapsed: vec!["docs".into()],
            seen_repos: vec!["api".into(), "docs".into()],
            collapsed_issues: vec!["I_227".into()],
            folded_comments: vec!["C_1".into()],
            selected: Some(Selected::Issue("I_5".into())),
            list_top: 3,
            detail_open: true,
            detail_focused: true,
            runs_tab: false,
            runs_group: "workflow".into(),
        }
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gh-issues-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_layout_round_trips_through_its_file() {
        let dir = tmp_dir("roundtrip");
        let path = path_in(&dir, "pgmac-net", None);
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path, "github"), Some(sample()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_view_started_in_a_clone_has_its_own_file() {
        let dir = Path::new("/s");
        assert_eq!(
            path_in(dir, "pgmac-net", None),
            Path::new("/s/pgmac-net.json")
        );
        assert_eq!(
            path_in(dir, "pgmac-net", Some("gh-issues-tui")),
            Path::new("/s/pgmac-net@gh-issues-tui.json")
        );
    }

    #[test]
    fn names_are_reduced_to_safe_file_name_characters() {
        let dir = Path::new("/s");
        assert_eq!(
            path_in(dir, "../../etc", Some("a/b c")),
            Path::new("/s/.._.._etc@a_b_c.json")
        );
        assert_eq!(path_in(dir, "..", None), Path::new("/s/_.json"));
        assert_eq!(path_in(dir, "", None), Path::new("/s/_.json"));
        // Whatever was passed, the file stays one component below the dir.
        assert_eq!(path_in(dir, "a/b", None).parent(), Some(dir));
    }

    #[test]
    fn a_missing_corrupt_old_or_foreign_file_is_ignored() {
        let dir = tmp_dir("ignored");
        let path = path_in(&dir, "o", None);
        assert_eq!(load(&path, "github"), None, "missing");

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load(&path, "github"), None, "corrupt");

        let old = Layout {
            version: VERSION - 1,
            ..sample()
        };
        save(&path, &old).unwrap();
        assert_eq!(load(&path, "github"), None, "older version");

        save(&path, &sample()).unwrap();
        assert_eq!(load(&path, "linear"), None, "another backend's ids");
        assert!(load(&path, "github").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_leaves_no_temp_file_behind() {
        let dir = tmp_dir("tmpfile");
        let path = path_in(&dir, "o", None);
        save(&path, &sample()).unwrap();
        save(&path, &sample()).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["o.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
