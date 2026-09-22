//! State for the in-app help viewer (#184).
//!
//! Pure, like the rest of `app/`: which topic is open, where closing returns
//! to, how far it is scrolled, and what the live-status block says. Geometry
//! (how many rows a page takes at a given width) is `ui::help`'s, and the
//! per-topic prose is `docs/help/*.md`, embedded at build time.

use super::prelude::*;

/// A page of the help viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTopic {
    /// The key table — generated from `LIST_HELP`, so it cannot drift.
    Keys,
    /// Semantic `/` search (#158).
    Search,
    /// The ticket-readiness badge (#160).
    Readiness,
    /// Inferred priority ranks and the `p` picker (#156, #162).
    Priority,
    /// Setup and disclosure for every TypeSafe feature.
    TypeSafe,
}

impl HelpTopic {
    /// In tab order.
    pub const ALL: [HelpTopic; 5] = [
        HelpTopic::Keys,
        HelpTopic::Search,
        HelpTopic::Readiness,
        HelpTopic::Priority,
        HelpTopic::TypeSafe,
    ];

    /// The name on the topic strip.
    pub fn title(self) -> &'static str {
        match self {
            HelpTopic::Keys => "keys",
            HelpTopic::Search => "search",
            HelpTopic::Readiness => "readiness",
            HelpTopic::Priority => "priority",
            HelpTopic::TypeSafe => "typesafe",
        }
    }

    /// The page's prose, straight from `docs/help/` so GitHub shows the same
    /// bytes the app does. `None` for `Keys`, which is generated.
    pub fn markdown(self) -> Option<&'static str> {
        match self {
            HelpTopic::Keys => None,
            HelpTopic::Search => Some(include_str!("../../../docs/help/search.md")),
            HelpTopic::Readiness => Some(include_str!("../../../docs/help/readiness.md")),
            HelpTopic::Priority => Some(include_str!("../../../docs/help/priority.md")),
            HelpTopic::TypeSafe => Some(include_str!("../../../docs/help/typesafe.md")),
        }
    }

    /// The topic `delta` places along the strip, wrapping.
    fn step(self, delta: isize) -> HelpTopic {
        let n = Self::ALL.len() as isize;
        let at = Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as isize;
        Self::ALL[(at + delta).rem_euclid(n) as usize]
    }
}

/// How one line of the status block reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    On,
    Off,
    Failed,
}

/// One row of the live-status block a help page opens with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub label: &'static str,
    pub value: String,
    pub tone: Tone,
}

/// The viewer's own state. The topic lives in `Mode::Help`.
#[derive(Debug)]
pub struct HelpState {
    /// The mode help was opened from, restored on close — so `F1` in the middle
    /// of typing a search comes back to that half-typed search.
    pub return_to: Mode,
    pub scroll: u16,
}

impl Default for HelpState {
    fn default() -> Self {
        Self {
            return_to: Mode::Normal,
            scroll: 0,
        }
    }
}

impl HelpState {
    /// Move the viewport, clamped to `0..=max`.
    pub fn scroll_by(&mut self, delta: i16, max: u16) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }
}

/// Filter-editor rows that have a help page of their own.
const TEXT_FIELD: usize = 0;
const PRIORITY_FIELD: usize = 4;

impl App {
    /// The page that best explains where the user is (#184).
    ///
    /// Anywhere not listed opens the key table, so `F1` always shows something
    /// rather than nothing.
    pub fn context_topic(&self) -> HelpTopic {
        match self.mode {
            Mode::Normal => {
                if self.detail.open
                    && self.focus == Focus::Detail
                    && self.selected_readiness().is_some()
                {
                    HelpTopic::Readiness
                } else {
                    HelpTopic::Keys
                }
            }
            Mode::Input(InputKind::Search) => HelpTopic::Search,
            Mode::Input(InputKind::FilterField(TEXT_FIELD)) => HelpTopic::Search,
            Mode::FilterMenu if self.filter_menu_idx == TEXT_FIELD => HelpTopic::Search,
            Mode::FilterMenu if self.filter_menu_idx == PRIORITY_FIELD => HelpTopic::Priority,
            Mode::SelectFieldMulti(PRIORITY_FIELD) => HelpTopic::Priority,
            Mode::PrioritySet | Mode::ConfirmPriority => HelpTopic::Priority,
            _ => HelpTopic::Keys,
        }
    }

    /// Open help on `topic`, remembering where to return to. Already open just
    /// changes page, so the return point is never help itself.
    pub fn open_help(&mut self, topic: HelpTopic) {
        if !matches!(self.mode, Mode::Help(_)) {
            self.help.return_to = self.mode;
        }
        self.help.scroll = 0;
        self.mode = Mode::Help(topic);
    }

    /// The `F12 ?` help, over a session: the session key table, not a topic.
    pub fn open_session_help(&mut self) {
        self.help.return_to = Mode::Harness;
        self.help.scroll = 0;
        self.mode = Mode::Help(HelpTopic::Keys);
    }

    /// Whether help is the session key table rather than the topic viewer.
    pub fn help_is_session(&self) -> bool {
        self.help.return_to == Mode::Harness
    }

    /// `F1`: open help for the current context, or close it if it is open.
    pub fn toggle_help(&mut self) {
        if matches!(self.mode, Mode::Help(_)) {
            self.close_help();
        } else {
            self.open_help(self.context_topic());
        }
    }

    /// Return to wherever help was opened from.
    pub fn close_help(&mut self) {
        if matches!(self.mode, Mode::Help(_)) {
            self.mode = self.help.return_to;
        }
    }

    /// Switch page along the strip, from the start.
    pub fn help_switch(&mut self, delta: isize) {
        if let Mode::Help(topic) = self.mode {
            self.help.scroll = 0;
            self.mode = Mode::Help(topic.step(delta));
        }
    }

    /// The live-status block a page opens with. Empty for pages with nothing
    /// to report about this session.
    pub fn help_status(&self, topic: HelpTopic) -> Vec<StatusLine> {
        let t = &self.typesafe;
        let search = self.feature(
            "semantic search",
            t.send_issue_text,
            "send_issue_text",
            self.search.has_failed(),
            "switching org (w) retries",
        );
        let readiness = self.feature(
            "readiness badge",
            t.send_issue_text,
            "send_issue_text",
            self.readiness.has_failed(),
            "retried after the next refresh",
        );
        let ranks = self.feature(
            "priority ranks",
            t.infer_priority_ranks,
            "infer_priority_ranks",
            self.label_rank.has_failed(),
            "switching org (w) retries",
        );
        let flag = |label: &'static str, on: bool| StatusLine {
            label,
            value: on.to_string(),
            tone: if on { Tone::On } else { Tone::Off },
        };
        match topic {
            HelpTopic::Keys => Vec::new(),
            HelpTopic::Search => vec![search],
            HelpTopic::Readiness => vec![readiness],
            HelpTopic::Priority => vec![ranks],
            HelpTopic::TypeSafe => vec![
                StatusLine {
                    label: "TYPESAFE_API_KEY",
                    value: if t.key_present { "set" } else { "not set" }.into(),
                    tone: if t.key_present { Tone::On } else { Tone::Off },
                },
                flag("send_issue_text", t.send_issue_text),
                flag("infer_priority_ranks", t.infer_priority_ranks),
                search,
                readiness,
                ranks,
            ],
        }
    }

    /// One feature's line: off for a flag or key that is missing, failed after
    /// an error, otherwise on. The flag is named first because it is the one
    /// the user chooses; the key usually is set once for many tools.
    fn feature(
        &self,
        label: &'static str,
        enabled: bool,
        flag: &str,
        failed: bool,
        retry: &str,
    ) -> StatusLine {
        let (tone, value) = if !enabled {
            (Tone::Off, format!("off — {flag} is false"))
        } else if !self.typesafe.key_present {
            (Tone::Off, "off — TYPESAFE_API_KEY is not set".to_string())
        } else if failed {
            (Tone::Failed, format!("off after an error — {retry}"))
        } else {
            (Tone::On, "on".to_string())
        };
        StatusLine { label, value, tone }
    }
}

/// The help pages state numbers, settings and file names that live in code.
/// These tests read the pages and hold them to it, so changing a threshold or
/// renaming a setting without updating help fails CI rather than shipping a page
/// that is quietly wrong (#184).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::typesafe::readiness::{self, Verdict};
    use crate::typesafe::{MIN_CONFIDENCE, search};

    fn page(topic: HelpTopic) -> &'static str {
        topic.markdown().expect("a prose page")
    }

    /// Lines inside ``` fences.
    fn fenced(md: &str) -> Vec<&str> {
        let mut inside = false;
        let mut out = Vec::new();
        for line in md.lines() {
            if line.trim_start().starts_with("```") {
                inside = !inside;
            } else if inside {
                out.push(line);
            }
        }
        out
    }

    #[test]
    fn the_filter_rows_the_context_table_names_are_the_right_ones() {
        assert_eq!(FILTER_FIELDS[TEXT_FIELD], "text");
        assert_eq!(FILTER_FIELDS[PRIORITY_FIELD], "priority");
    }

    #[test]
    fn every_prose_topic_has_a_page_and_every_page_has_a_topic() {
        let mut on_disk: Vec<String> =
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/help"))
                .unwrap()
                .map(|e| {
                    let name = e.unwrap().file_name().into_string().unwrap();
                    name.strip_suffix(".md")
                        .unwrap_or_else(|| panic!("{name}: docs/help holds only .md pages"))
                        .to_string()
                })
                .collect();
        on_disk.sort();

        let mut topics: Vec<String> = HelpTopic::ALL
            .iter()
            .filter(|t| t.markdown().is_some())
            .map(|t| t.title().to_string())
            .collect();
        topics.sort();
        assert_eq!(on_disk, topics, "an orphaned page, or a topic with none");
        assert_eq!(HelpTopic::Keys.markdown(), None, "the keys are generated");
    }

    #[test]
    fn a_page_is_plain_markdown_the_help_can_render() {
        for topic in HelpTopic::ALL
            .iter()
            .filter_map(|t| t.markdown().map(|_| *t))
        {
            let md = page(topic);
            assert!(md.starts_with("# "), "{topic:?} opens with a heading");
            // Help does not follow links, so a link would show its label and
            // silently lose its target.
            assert!(!md.contains("]("), "{topic:?} must not contain links");
            assert!(md.ends_with('\n'), "{topic:?}");
        }
    }

    #[test]
    fn search_states_the_threshold_and_the_body_length_the_code_uses() {
        let md = page(HelpTopic::Search);
        assert!(
            md.contains(&format!("above {:.2}", search::SEARCH_YES)),
            "threshold: SEARCH_YES = {}",
            search::SEARCH_YES
        );
        assert!(
            md.contains(&format!("first {} characters", search::BODY_CHARS)),
            "body length: BODY_CHARS = {}",
            search::BODY_CHARS
        );
        // What the info bar says, so a reworded indicator updates the page.
        for text in ["semantic: searching…", "semantic: +", "semantic: off ("] {
            assert!(md.contains(text), "{text}");
        }
    }

    #[test]
    fn readiness_states_what_is_sent_and_every_verdict() {
        let md = page(HelpTopic::Readiness);
        assert!(md.contains(&format!("first {} characters", readiness::BODY_CHARS)));
        assert!(md.contains(&format!("last {} comments", readiness::COMMENTS_SENT)));
        assert!(md.contains(&format!("{} characters each", readiness::COMMENT_CHARS)));

        use readiness::Signal;
        for verdict in [
            Verdict::Ready,
            Verdict::Thin(vec![Signal::Specifics]),
            Verdict::Blocked,
            Verdict::AlreadyCovered,
            Verdict::NotActionable,
            Verdict::MaybeBlocked,
            Verdict::MaybeDuplicate,
            Verdict::Unclear(vec![Signal::Specifics]),
        ] {
            let line = verdict.line();
            let head = line.split(" \u{2014} ").next().unwrap();
            assert!(
                md.contains(head),
                "{head:?} is a verdict the page must explain"
            );
        }
    }

    #[test]
    fn priority_states_the_confidence_gate_and_the_cache_file() {
        let md = page(HelpTopic::Priority);
        assert!(
            md.contains(&format!("at least {MIN_CONFIDENCE} confident")),
            "MIN_CONFIDENCE = {MIN_CONFIDENCE}"
        );
        let cache = crate::typesafe::cache::default_path();
        let file = cache.file_name().unwrap().to_str().unwrap();
        assert!(md.contains(&format!("~/.cache/gh-issues/{file}")));
    }

    /// Every setting a page puts in a config block is one `Config` really reads,
    /// and every page that names the key names the one the code reads.
    #[test]
    fn the_settings_and_key_the_pages_name_are_the_ones_the_code_reads() {
        let cfg: crate::config::Config =
            toml::from_str("send_issue_text = true\ninfer_priority_ranks = true").unwrap();
        assert!(cfg.send_issue_text && cfg.infer_priority_ranks);

        for topic in HelpTopic::ALL
            .iter()
            .filter_map(|t| t.markdown().map(|_| *t))
        {
            for line in fenced(page(topic)) {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("export ") {
                    let var = rest.split('=').next().unwrap();
                    assert_eq!(var, crate::typesafe::API_KEY_ENV, "{topic:?}");
                } else if !line.starts_with('#') && line.contains(" = ") {
                    let name = line.split(" = ").next().unwrap();
                    assert!(
                        matches!(name, "send_issue_text" | "infer_priority_ranks"),
                        "{topic:?}: `{name}` is not a setting the code reads"
                    );
                }
            }
        }
    }

    /// A page names the setting a feature needs; the wrong one would send the
    /// user off to enable something that does not enable it.
    #[test]
    fn each_feature_page_names_the_setting_that_gates_it() {
        assert!(page(HelpTopic::Search).contains("send_issue_text = true"));
        assert!(page(HelpTopic::Readiness).contains("send_issue_text = true"));
        assert!(page(HelpTopic::Priority).contains("infer_priority_ranks = true"));
        assert!(!page(HelpTopic::Priority).contains("send_issue_text"));
        let ts = page(HelpTopic::TypeSafe);
        assert!(ts.contains("send_issue_text") && ts.contains("infer_priority_ranks"));
    }

    /// ADR 0004 clause 3, as the user is told it: each disclosure page carries a
    /// sentence saying the org (and, for the text features, the repo) never leaves.
    #[test]
    fn the_pages_say_the_org_and_repo_never_leave() {
        let says = |md: &str, words: &[&str]| {
            md.lines().any(|l| {
                let l = l.to_lowercase();
                l.contains("never") && words.iter().all(|w| l.contains(w))
            })
        };
        for topic in [HelpTopic::Search, HelpTopic::Readiness, HelpTopic::TypeSafe] {
            assert!(
                says(page(topic), &["org", "repo"]),
                "{topic:?} must say the org and repo are never sent"
            );
        }
        assert!(says(page(HelpTopic::Priority), &["org"]));
    }
}
