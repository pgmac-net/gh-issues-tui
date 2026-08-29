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
    /// Set when this session was dispatched via `bg_dispatch`: the id of the
    /// *actual* background session, as opposed to the local PTY child (a
    /// `claude attach` viewer) tracked above. `kill` stops this instead of
    /// the viewer, and `kill_all` (quit) leaves it running rather than
    /// stopping it — the PTY is only ever a window onto it.
    bg_id: Option<String>,
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
    /// The issue's title, carried so the session's identity row can name the
    /// ticket rather than only numbering it (#132). Deliberately not an argv
    /// placeholder: it is display text, and it is long.
    pub title: String,
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

/// Where the child came from (#132), as env var pairs rather than stamped
/// straight onto a builder — `bg_dispatch` needs these on the *dispatch*
/// process (`std::process::Command`) while a direct exec needs them on the
/// PTY's `CommandBuilder`, and both should stay in lock-step with one list.
///
/// The agent already learns its *ticket* from argv — the builtin `claude`
/// entry passes `/pgmac-workflows:pickup-ticket {ref}` as the prompt. What it
/// cannot learn that way is its *launcher*, and neither can anything the agent
/// itself spawns: hooks, statuslines and scripts would otherwise have to
/// scrape the parent's argv, which is formatted differently by every harness
/// and carries no issue reference at all for `opencode`.
///
/// `GH_ISSUES_TUI` doubles as the marker and the version, so a hook can both
/// detect the launcher and branch on it with one variable.
///
/// Callers set these through an env-setting method that never invokes a
/// shell — issue titles containing `$(…)` or backticks stay inert, the same
/// property the argv array gives `expand_argv`.
fn provenance_env(harness_name: &str, ctx: &LaunchContext) -> [(&'static str, String); 8] {
    [
        ("GH_ISSUES_TUI", env!("CARGO_PKG_VERSION").to_string()),
        ("GH_ISSUES_TUI_HARNESS", harness_name.to_string()),
        ("GH_ISSUES_TUI_OWNER", ctx.owner.clone()),
        ("GH_ISSUES_TUI_REPO", ctx.repo.clone()),
        ("GH_ISSUES_TUI_NUMBER", ctx.number.to_string()),
        ("GH_ISSUES_TUI_ISSUE", ctx.issue_ref()),
        ("GH_ISSUES_TUI_URL", ctx.url.clone()),
        ("GH_ISSUES_TUI_TITLE", ctx.title.clone()),
    ]
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

/// Run `argv` to completion in `cwd`, off the PTY — used for the `bg_dispatch`
/// step, which only needs to hand off to the supervisor and exit. Its own
/// stdout is not parsed: the freshly named session is looked up afterward via
/// `claude agents --json`, a stable, JSON-structured format rather than a
/// human-readable line guessed at.
///
/// Captured with `.output()`, not inherited: `claude --bg` prints its own
/// "backgrounded · id · name / claude attach … / claude logs … / claude
/// stop …" banner, and gh-issues-tui has the terminal in raw mode for the
/// TUI — an inherited child writing straight to it corrupts the frame until
/// the next full redraw. Only surfaced (in `stderr`) if the dispatch failed.
fn run_to_completion(
    argv: &[String],
    cwd: &Path,
    harness_name: &str,
    ctx: &LaunchContext,
) -> Result<(), String> {
    let Some((program, args)) = argv.split_first() else {
        return Err("bg_dispatch command is empty".to_string());
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env("TERM", "xterm-256color");
    // This process is the actual agent (the PTY below only ever attaches a
    // viewer to it), so it — not the viewer — gets the provenance env.
    for (k, v) in provenance_env(harness_name, ctx) {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("starting {program} failed: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{program} exited with {}: {}",
            output.status,
            stderr.trim()
        ))
    }
}

/// One row of `claude agents --json` — just enough to match a session back
/// to the issue ref it was dispatched with, and to notice one that already
/// finished (`state`) before `spawn` even got to attach a viewer.
#[derive(serde::Deserialize)]
pub struct BgSession {
    pub id: String,
    pub name: Option<String>,
    /// `"working"` while the agent has an active turn, `"done"` once it
    /// hasn't — the shape observed live from `claude agents --json`, not
    /// otherwise documented. `spawn` uses this only as a coarse "finished
    /// unusually fast" signal, never to guess *why*.
    pub state: Option<String>,
}

/// List currently-running background sessions. Deliberately not `--all`: a
/// session that already stopped falls out of the default listing, so this
/// (and `find_bg_agent_by_name`, built on it) never returns a stale hit from
/// an earlier run that happened to reuse the same `--name`.
///
/// Used both right after dispatch (to resolve the id `spawn` just started)
/// and at `gh-issues-tui` startup (to adopt sessions still running from a
/// previous run — see `HarnessState::reconcile` in `tui::app::harness`).
pub fn list_bg_sessions() -> Result<Vec<BgSession>, String> {
    let output = std::process::Command::new("claude")
        .args(["agents", "--json"])
        .output()
        .map_err(|e| format!("listing background agents failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "claude agents --json exited with {}",
            output.status
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parsing background agent list failed: {e}"))
}

fn find_bg_agent_by_name(name: &str) -> Result<BgSession, String> {
    list_bg_sessions()?
        .into_iter()
        .find(|a| a.name.as_deref() == Some(name))
        .ok_or_else(|| {
            format!("no background session named {name} (it may have exited immediately)")
        })
}

/// Stop a background session directly by its id, with no local PTY required
/// — the path for a session adopted from `claude agents` at startup that was
/// never attached in this process, so `HarnessRegistry::kill` has no
/// `LiveSession` to find it through.
///
/// `.output()`, not inherited: `claude stop` prints its own "stopped <id>"
/// line, and this runs while the TUI has the terminal in raw mode.
pub fn stop_bg(bg_id: &str) {
    let _ = std::process::Command::new("claude")
        .args(["stop", bg_id])
        .output();
}

impl HarnessRegistry {
    /// Start `harness` for `ctx` under `id`, on a PTY sized to `(rows, cols)`.
    ///
    /// When `harness.bg_dispatch` is set, this first runs it to completion
    /// (not on the PTY) to start a real background session under Claude
    /// Code's own supervisor, looks that session up by name, and only then
    /// opens the PTY — against `claude attach <id>`, not the original
    /// prompt. The PTY is a viewer onto that session, not the session
    /// itself, which is what lets it outlive this process.
    ///
    /// Two threads are spawned per session: one draining the PTY into the
    /// parser, one waiting on the child. Both report through `tx` and exit on
    /// their own when the child goes away.
    ///
    /// Returns whether the dispatched session had already finished its first
    /// turn (`state: "done"`, not `"working"`) by the time it was looked up —
    /// always `false` for a direct-exec harness, which has no such state to
    /// check. A real ticket workflow shouldn't finish before this process has
    /// even attached to it; this is not a guess at *why* it finished, only
    /// that it did, which the caller can use to nudge the user to look.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        id: SessionId,
        harness_name: &str,
        harness: &HarnessConfig,
        ctx: &LaunchContext,
        cwd: &Path,
        pane: Rect,
        tx: &mpsc::UnboundedSender<AppEvent>,
    ) -> Result<bool, String> {
        let (argv, bg_id, finished_fast) = match &harness.bg_dispatch {
            Some(dispatch) => {
                let dispatch_argv = expand_argv(dispatch, ctx);
                run_to_completion(&dispatch_argv, cwd, harness_name, ctx)?;
                let bg = find_bg_agent_by_name(&ctx.issue_ref())?;
                let finished_fast = bg.state.as_deref() == Some("done");
                let attach_argv = expand_argv(&harness.command, ctx)
                    .into_iter()
                    .map(|arg| arg.replace("{bg_id}", &bg.id))
                    .collect();
                (attach_argv, Some(bg.id), finished_fast)
            }
            None => (expand_argv(&harness.command, ctx), None, false),
        };
        // The dispatch process above is the actual agent for a bg_dispatch
        // harness — this PTY is only ever a `claude attach` viewer onto it —
        // but stamping provenance here too is harmless and keeps every
        // direct-exec harness (opencode, or a user override with no
        // bg_dispatch) getting it exactly as before.
        self.open_pty(
            id,
            &argv,
            cwd,
            pane,
            &provenance_env(harness_name, ctx),
            tx,
            bg_id,
        )?;
        Ok(finished_fast)
    }

    /// Open a PTY viewer onto `bg_id`, a background session this process did
    /// not itself dispatch — found via `claude agents` and adopted into
    /// `HarnessState` at startup with no PTY yet, only metadata. Only
    /// `{bg_id}` is expanded in `attach_template` (the harness's `command`):
    /// the local viewer needs none of `expand_argv`'s other placeholders.
    pub fn attach_bg(
        &mut self,
        id: SessionId,
        bg_id: &str,
        attach_template: &[String],
        cwd: &Path,
        pane: Rect,
        tx: &mpsc::UnboundedSender<AppEvent>,
    ) -> Result<(), String> {
        let argv: Vec<String> = attach_template
            .iter()
            .map(|arg| arg.replace("{bg_id}", bg_id))
            .collect();
        self.open_pty(id, &argv, cwd, pane, &[], tx, Some(bg_id.to_string()))
    }

    /// Open `argv` on a fresh PTY sized to `pane` and track it under `id`.
    /// Shared by `spawn` (a fresh launch, dispatched or direct) and
    /// `attach_bg` (adopting an already-running background session) — both
    /// just need a PTY viewer onto some process; only how that process's argv
    /// and env are decided differs.
    #[allow(clippy::too_many_arguments)]
    fn open_pty(
        &mut self,
        id: SessionId,
        argv: &[String],
        cwd: &Path,
        pane: Rect,
        env: &[(&'static str, String)],
        tx: &mpsc::UnboundedSender<AppEvent>,
        bg_id: Option<String>,
    ) -> Result<(), String> {
        let (rows, cols) = (pane.height, pane.width);
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
        for (k, v) in env {
            cmd.env(k, v);
        }

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
                bg_id,
            },
        );
        Ok(())
    }

    /// A session's parser, for the renderer.
    pub fn parser(&self, id: SessionId) -> Option<&Arc<Mutex<vt100::Parser>>> {
        self.map.get(&id).map(|s| &s.parser)
    }

    /// The background session id behind `id`, when it was started via
    /// `bg_dispatch` rather than execed directly — `None` for a harness with
    /// no such split (e.g. `opencode`), and for an id with no live PTY at all.
    pub fn bg_id(&self, id: SessionId) -> Option<&str> {
        self.map.get(&id).and_then(|s| s.bg_id.as_deref())
    }

    /// True once `spawn` (or `attach_bg`) has given `id` a live PTY in this
    /// process. A session adopted from `claude agents` at startup has none
    /// until it is actually attached.
    pub fn has_pty(&self, id: SessionId) -> bool {
        self.map.contains_key(&id)
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
    ///
    /// For a `bg_dispatch` session, killing the local `claude attach` viewer
    /// alone would only close the window onto it — the background session
    /// would keep running under the supervisor. `claude stop` is what
    /// actually ends it; the viewer is killed too so the pane doesn't sit on
    /// a session it can no longer reach.
    pub fn kill(&mut self, id: SessionId) {
        if let Some(session) = self.map.get_mut(&id) {
            if let Some(bg_id) = &session.bg_id {
                stop_bg(bg_id);
            }
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
            title: "Send to Claude/harness".into(),
        }
    }

    #[test]
    fn bg_session_deserializes_the_real_claude_agents_json_shape() {
        // Pinned against a live `claude agents --json` sample, not guessed —
        // see the plan/session notes this test was added from.
        let json = r#"[
            {
                "pid": 56552,
                "id": "61db7f48",
                "cwd": "/home/paul/pgmac",
                "kind": "background",
                "startedAt": 1787997678667,
                "sessionId": "8c1d9143-1ea7-4480-b337-899f0de2768d",
                "name": "reduce-arc-runner-churn-dqlite",
                "status": "busy",
                "state": "working"
            }
        ]"#;
        let sessions: Vec<BgSession> = serde_json::from_str(json).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "61db7f48");
        assert_eq!(
            sessions[0].name.as_deref(),
            Some("reduce-arc-runner-churn-dqlite")
        );
        assert_eq!(sessions[0].state.as_deref(), Some("working"));
    }

    #[test]
    fn bg_session_tolerates_missing_optional_fields() {
        // Extra unknown fields (pid, cwd, …) are ignored by default; a
        // response missing `name`/`state` must not fail to parse.
        let json = r#"[{"id": "abc123"}]"#;
        let sessions: Vec<BgSession> = serde_json::from_str(json).unwrap();
        assert_eq!(sessions[0].id, "abc123");
        assert_eq!(sessions[0].name, None);
        assert_eq!(sessions[0].state, None);
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
            title: "$(id) `whoami` ; rm -rf ~".into(),
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
        // Expected text is built with the same path machinery rather than
        // spelled out: Windows joins with `\`, so hardcoding `/nope/one/myrepo`
        // asserted the separator instead of the property under test.
        for root in &roots {
            let expected = expand_tilde(root).join("myrepo").display().to_string();
            assert!(err.contains(&expected), "{expected} missing from {err}");
        }
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
            bg_dispatch: None,
        };
        registry
            .spawn(
                0,
                "test-harness",
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
