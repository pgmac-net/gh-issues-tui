//! Harness sessions — the impure half of the feature (#23).
//!
//! Owns everything `app/harness.rs` deliberately does not: the PTYs, the
//! child processes, the reader threads and the `vt100` parsers. The registry
//! is held by the event loop, never by `App`, so `app/` keeps its no-I/O
//! invariant and every state transition stays unit-testable.
//!
//! **Output never travels through the event channel.** A reader thread feeds
//! bytes straight into its session's parser behind a mutex and sends only a
//! zero-payload [`AppEvent::HarnessDirty`], so a chatty agent redrawing at
//! full tilt cannot outrun or stall the `tokio::select!` loop.

pub mod keys;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::config::HarnessConfig;
use crate::tui::app::SessionId;
use crate::tui::event::AppEvent;

/// Rows of scrollback kept per session. An agent's run easily exceeds a
/// screen, and the whole point of keeping exited sessions is being able to
/// read back what it did.
const SCROLLBACK: usize = 5000;

/// The live half of a session: what it takes to draw it, type at it, resize
/// it and kill it.
struct LiveSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

/// Every running or exited session's PTY, keyed by the id handed out by
/// `HarnessState::register`.
#[derive(Default)]
pub struct HarnessRegistry {
    map: HashMap<SessionId, LiveSession>,
}

/// The config the harness feature needs, resolved once at startup so the
/// event loop never reaches back into `Config`.
pub struct HarnessSettings {
    pub default_harness: Option<String>,
    pub harnesses: HashMap<String, HarnessConfig>,
    pub workspace_roots: Vec<String>,
    /// `(owner, repo)` of the directory the TUI was started in, when it is a
    /// GitHub clone. Lets a launch for that repo skip the root search.
    pub cwd_repo: Option<(String, String)>,
    pub cwd: PathBuf,
}

impl HarnessSettings {
    pub fn get(&self, name: &str) -> Option<&HarnessConfig> {
        self.harnesses.get(name)
    }

    /// Harness names in display order, for the picker.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.harnesses.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    /// Roots to search for `name`'s workspace: its own override, else the
    /// top-level list.
    pub fn roots_for(&self, name: &str) -> &[String] {
        self.get(name)
            .and_then(|h| h.workspace_roots.as_deref())
            .unwrap_or(&self.workspace_roots)
    }

    /// Where a harness launched for `owner/repo` should run.
    pub fn workspace(&self, name: &str, owner: &str, repo: &str) -> Result<PathBuf, String> {
        resolve_workspace(
            repo,
            owner,
            self.cwd_repo.as_ref(),
            &self.cwd,
            self.roots_for(name),
        )
    }
}

/// Everything needed to expand a harness command for one issue.
pub struct LaunchContext {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
}

impl LaunchContext {
    /// Canonical `owner/repo#number`, matching `App::selected_issue_ref`.
    pub fn issue_ref(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// Expand the `{owner}`, `{repo}`, `{number}`, `{ref}` and `{url}`
/// placeholders across an argv array.
///
/// Each element is expanded independently and stays one argv slot, so issue
/// text containing quotes, `$(…)` or backticks is inert — there is no shell
/// anywhere in this path.
pub fn expand_argv(command: &[String], ctx: &LaunchContext) -> Vec<String> {
    let issue_ref = ctx.issue_ref();
    let number = ctx.number.to_string();
    command
        .iter()
        .map(|arg| {
            arg.replace("{owner}", &ctx.owner)
                .replace("{repo}", &ctx.repo)
                .replace("{number}", &number)
                .replace("{ref}", &issue_ref)
                .replace("{url}", &ctx.url)
        })
        .collect()
}

/// Locate the clone a harness should run in.
///
/// `cwd_repo` is the `(owner, repo)` of the directory the TUI was started in,
/// when it is a GitHub clone; that wins so launching from inside the repo
/// always does the obvious thing. Otherwise the first existing
/// `<root>/<repo>` across `roots` is taken, in order.
pub fn resolve_workspace(
    repo: &str,
    owner: &str,
    cwd_repo: Option<&(String, String)>,
    cwd: &Path,
    roots: &[String],
) -> Result<PathBuf, String> {
    if let Some((cwd_owner, cwd_name)) = cwd_repo
        && cwd_owner.eq_ignore_ascii_case(owner)
        && cwd_name.eq_ignore_ascii_case(repo)
    {
        return Ok(cwd.to_path_buf());
    }
    let mut tried = Vec::new();
    for root in roots {
        let candidate = expand_tilde(root).join(repo);
        if candidate.is_dir() {
            return Ok(candidate);
        }
        tried.push(candidate.display().to_string());
    }
    Err(if tried.is_empty() {
        format!("no clone of {owner}/{repo}: set `workspace_roots` in the config file")
    } else {
        format!(
            "no clone of {owner}/{repo} (looked in {})",
            tried.join(", ")
        )
    })
}

/// Expand a leading `~` against the home directory. Any other path is taken
/// as-is, so absolute and relative roots both work.
pub fn expand_tilde(path: &str) -> PathBuf {
    let Some(rest) = path.strip_prefix('~') else {
        return PathBuf::from(path);
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    match dirs::home_dir() {
        Some(home) if rest.is_empty() => home,
        Some(home) => home.join(rest),
        None => PathBuf::from(path),
    }
}

impl HarnessRegistry {
    /// Start `harness` for `ctx` under `id`, on a PTY sized to `(rows, cols)`.
    ///
    /// Two threads are spawned per session: one draining the PTY into the
    /// parser, one waiting on the child. Both report through `tx` and exit on
    /// their own when the child goes away.
    pub fn spawn(
        &mut self,
        id: SessionId,
        harness: &HarnessConfig,
        ctx: &LaunchContext,
        cwd: &Path,
        pane: Rect,
        tx: &mpsc::UnboundedSender<AppEvent>,
    ) -> Result<(), String> {
        let (rows, cols) = (pane.height, pane.width);
        let argv = expand_argv(&harness.command, ctx);
        let Some((program, args)) = argv.split_first() else {
            return Err("harness command is empty".to_string());
        };

        let size = pty_size(rows, cols);
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| format!("opening a pty failed: {e}"))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        // The child inherits the parent environment (tokens, PATH); TERM is
        // pinned because vt100 speaks xterm and the outer terminal's TERM
        // may name something it does not implement.
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("starting {program} failed: {e}"))?;
        // The slave fd must not outlive the spawn, or the reader below never
        // sees EOF when the child exits — it would stay open in this process.
        drop(pair.slave);

        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("reading the pty failed: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("writing to the pty failed: {e}"))?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));

        spawn_reader(id, reader, Arc::clone(&parser), tx.clone());
        spawn_waiter(id, child, tx.clone());

        self.map.insert(
            id,
            LiveSession {
                master: pair.master,
                writer,
                parser,
                killer,
            },
        );
        Ok(())
    }

    /// A session's parser, for the renderer.
    pub fn parser(&self, id: SessionId) -> Option<&Arc<Mutex<vt100::Parser>>> {
        self.map.get(&id).map(|s| &s.parser)
    }

    /// Send raw bytes to a child. Errors are swallowed: a child that exited
    /// between the keypress and the write is normal, and its session is
    /// about to be marked exited anyway.
    pub fn write(&mut self, id: SessionId, bytes: &[u8]) {
        if let Some(session) = self.map.get_mut(&id) {
            let _ = session.writer.write_all(bytes);
            let _ = session.writer.flush();
        }
    }

    /// Resize a session's PTY and its parser together, so the child's idea of
    /// the screen and ours cannot drift.
    pub fn resize(&mut self, id: SessionId, rows: u16, cols: u16) {
        let Some(session) = self.map.get_mut(&id) else {
            return;
        };
        let _ = session.master.resize(pty_size(rows, cols));
        if let Ok(mut parser) = session.parser.lock() {
            parser.screen_mut().set_size(rows, cols);
        }
    }

    /// Scroll an exited session's frozen screen back by `rows`. Kept here
    /// rather than in the renderer so `ui/` stays a pure function of state.
    pub fn set_scrollback(&mut self, id: SessionId, rows: usize) {
        if let Some(session) = self.map.get_mut(&id)
            && let Ok(mut parser) = session.parser.lock()
        {
            parser.screen_mut().set_scrollback(rows);
        }
    }

    /// Ask a child to terminate. The `HarnessExited` event still arrives from
    /// the waiter thread, so the session is marked exited exactly once
    /// however it died.
    pub fn kill(&mut self, id: SessionId) {
        if let Some(session) = self.map.get_mut(&id) {
            let _ = session.killer.kill();
        }
    }

    /// Forget a session, closing its PTY. Any child still attached receives
    /// SIGHUP as the master closes.
    pub fn remove(&mut self, id: SessionId) {
        self.map.remove(&id);
    }

    /// Terminate everything, for quit.
    pub fn kill_all(&mut self) {
        for session in self.map.values_mut() {
            let _ = session.killer.kill();
        }
        self.map.clear();
    }
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Drain the PTY into the parser until EOF, nudging the UI as output arrives.
fn spawn_reader(
    id: SessionId,
    mut reader: Box<dyn Read + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            // A closed slave surfaces as Ok(0) or EIO depending on platform;
            // both mean the child is gone and this thread is done.
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if let Ok(mut parser) = parser.lock() {
                parser.process(&buf[..n]);
            }
            if tx.send(AppEvent::HarnessDirty(id)).is_err() {
                break; // the TUI is gone
            }
        }
    });
}

/// Wait on the child and report how it ended.
fn spawn_waiter(
    id: SessionId,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    std::thread::spawn(move || {
        // A child killed by a signal has no exit code; -1 stands in, and the
        // status line reports it as such.
        let code = match child.wait() {
            Ok(status) => status.exit_code() as i32,
            Err(_) => -1,
        };
        let _ = tx.send(AppEvent::HarnessExited { id, code });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> LaunchContext {
        LaunchContext {
            owner: "pgmac-net".into(),
            repo: "gh-issues-tui".into(),
            number: 23,
            url: "https://github.com/pgmac-net/gh-issues-tui/issues/23".into(),
        }
    }

    #[test]
    fn expands_every_placeholder() {
        let cmd = vec![
            "agent".into(),
            "{owner}/{repo}".into(),
            "#{number}".into(),
            "{ref}".into(),
            "{url}".into(),
        ];
        assert_eq!(
            expand_argv(&cmd, &ctx()),
            vec![
                "agent",
                "pgmac-net/gh-issues-tui",
                "#23",
                "pgmac-net/gh-issues-tui#23",
                "https://github.com/pgmac-net/gh-issues-tui/issues/23",
            ]
        );
    }

    #[test]
    fn a_placeholder_expands_within_a_larger_argument() {
        let cmd = vec![
            "claude".into(),
            "/pgmac-workflows:pickup-ticket {ref}".into(),
        ];
        assert_eq!(
            expand_argv(&cmd, &ctx())[1],
            "/pgmac-workflows:pickup-ticket pgmac-net/gh-issues-tui#23"
        );
    }

    #[test]
    fn expansion_never_splits_an_argument() {
        // The security property: whatever an issue reference expands to, it
        // stays exactly one argv slot. No shell, no word splitting.
        let ctx = LaunchContext {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
            url: "https://x/$(touch /tmp/pwned) `id` ; rm -rf ~".into(),
        };
        let argv = expand_argv(&["agent".into(), "{url}".into()], &ctx);
        assert_eq!(argv.len(), 2, "the url must not become several arguments");
        assert!(
            argv[1].contains("$(touch /tmp/pwned)"),
            "passed through inert"
        );
    }

    #[test]
    fn an_argument_without_placeholders_is_untouched() {
        let argv = expand_argv(&["opencode".into(), "run".into()], &ctx());
        assert_eq!(argv, vec!["opencode", "run"]);
    }

    #[test]
    fn workspace_prefers_the_cwd_when_it_is_the_issue_s_repo() {
        let cwd = Path::new("/somewhere/gh-issues-tui");
        let cwd_repo = ("pgmac-net".to_string(), "gh-issues-tui".to_string());
        let found = resolve_workspace(
            "gh-issues-tui",
            "pgmac-net",
            Some(&cwd_repo),
            cwd,
            &["/nonexistent".to_string()],
        );
        assert_eq!(found.unwrap(), cwd);
    }

    #[test]
    fn a_cwd_for_a_different_repo_does_not_win() {
        let cwd_repo = ("pgmac-net".to_string(), "other".to_string());
        let err = resolve_workspace(
            "gh-issues-tui",
            "pgmac-net",
            Some(&cwd_repo),
            Path::new("/somewhere/other"),
            &[],
        )
        .unwrap_err();
        assert!(err.contains("workspace_roots"), "got {err}");
    }

    #[test]
    fn workspace_takes_the_first_root_that_exists() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("myrepo")).unwrap();
        let roots = vec![
            first.path().display().to_string(),
            second.path().display().to_string(),
        ];
        let found = resolve_workspace("myrepo", "o", None, Path::new("/"), &roots).unwrap();
        assert_eq!(found, second.path().join("myrepo"));
    }

    #[test]
    fn a_missing_clone_reports_every_path_tried() {
        let roots = vec!["/nope/one".to_string(), "/nope/two".to_string()];
        let err = resolve_workspace("myrepo", "o", None, Path::new("/"), &roots).unwrap_err();
        assert!(err.contains("/nope/one/myrepo"), "got {err}");
        assert!(err.contains("/nope/two/myrepo"), "got {err}");
    }

    #[test]
    fn tilde_expands_to_the_home_directory() {
        let home = dirs::home_dir().expect("a home directory");
        assert_eq!(expand_tilde("~/pgmac"), home.join("pgmac"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("relative"), PathBuf::from("relative"));
    }

    /// The one test that spawns a real process: proves the PTY path actually
    /// works end to end, which no amount of pure testing can.
    #[test]
    #[cfg(unix)]
    fn spawns_a_child_reads_its_output_and_reports_the_exit_code() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut registry = HarnessRegistry::default();
        let harness = HarnessConfig {
            command: vec!["sh".into(), "-c".into(), "echo hello-{repo}; exit 3".into()],
            workspace_roots: None,
        };
        registry
            .spawn(
                0,
                &harness,
                &ctx(),
                Path::new("/"),
                Rect::new(0, 0, 80, 24),
                &tx,
            )
            .expect("spawn");

        // `HarnessExited` comes from the waiter thread and says nothing about
        // whether the reader thread has finished draining the pty — stopping
        // there raced the final output and made this test flaky (~1 in 40).
        // The app is unaffected (a later `HarnessDirty` redraws), but the
        // assertion below needs both threads done. Dropping the test's own
        // sender leaves the two thread clones, so the channel closes exactly
        // when both have exited.
        drop(tx);
        let mut code = None;
        while let Some(event) = rx.blocking_recv() {
            if let AppEvent::HarnessExited { id, code: c } = event {
                assert_eq!(id, 0);
                code = Some(c);
            }
        }
        assert_eq!(code, Some(3), "exit code must reach the event loop");

        let parser = registry.parser(0).expect("parser").lock().unwrap();
        let screen: String = parser.screen().contents();
        assert!(
            screen.contains("hello-gh-issues-tui"),
            "placeholders expanded and output parsed; got {screen:?}"
        );
    }
}
