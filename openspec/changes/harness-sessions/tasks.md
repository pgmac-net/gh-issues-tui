## 1. Dependencies and config

- [x] 1.1 Add `tui-term` 0.3, `portable-pty` 0.9, `vt100` 0.16; confirm a single `vt100` resolves against `tui-term`'s
- [x] 1.2 Add `default_harness`, `workspace_roots` and the `[harnesses.*]` table to `Config`
- [x] 1.3 Merge built-in harnesses per-name after parse, so defining one does not delete the others

## 2. Pure state

- [x] 2.1 `app/harness.rs`: `HarnessState`, `SessionMeta`, `SessionStatus`, `SessionId`
- [x] 2.2 `LaunchAction` and `App::launch_action` — what `A` does, computed purely
- [x] 2.3 `App::selected_issue_ref` — canonical `owner/repo#number`, independent of `copy_format`
- [x] 2.4 Add `Mode::Harness`, `HarnessPicker`, `SessionPicker`, `ConfirmHarness(HarnessConfirm)`

## 3. Runtime

- [x] 3.1 `tui/harness/mod.rs`: registry, spawn, reader and waiter threads
- [x] 3.2 Placeholder expansion over argv, and workspace resolution across roots
- [x] 3.3 `tui/harness/keys.rs`: crossterm `KeyEvent` → xterm byte encoder
- [x] 3.4 `HarnessSettings` resolved once in `main.rs`

## 4. Wiring

- [x] 4.1 `AppEvent::HarnessDirty`/`HarnessExited`; registry owned by the event loop
- [x] 4.2 Skip the redraw when a dirty session is not on screen
- [x] 4.3 Resize every session's PTY on terminal resize
- [x] 4.4 `layout::harness_areas`, guarding against a zero-row pane
- [x] 4.5 `A`, `Z` and quit gating in normal mode; `F12` chord in `Mode::Harness`
- [x] 4.6 Render via `PseudoTerminal` + status row; extract the shared Yes/No confirm popup

## 5. Verification

- [x] 5.1 Pure tests: expansion, workspace fallback, registry transitions, chord machine, key encoding
- [x] 5.2 One `#[cfg(unix)]` test spawning a real child through a PTY
- [x] 5.3 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green (500 tests)
- [x] 5.4 End-to-end under tmux: launch, detach, two concurrent sessions, switch, exit, relaunch, quit-confirm, resize
- [x] 5.5 Fix found by 5.4: `F12 n` bypassed the one-session-per-issue rule

## 6. Documentation

- [x] 6.1 `docs/harness-sessions.md`
- [x] 6.2 README key table and config reference, including the unverified-harness snippets
- [x] 6.3 `CLAUDE.md` invariants
