# 1. Adoption is name-shaped; ownership is surfaced, not enforced

## Status

Accepted (#148)

## Context

`HarnessState::reconcile` adopts a still-running `claude --bg` session at startup so a
session survives `gh-issues-tui` quitting and being relaunched. It matches on the
session's `--name` alone: if it parses as `owner/repo#number` (`is_issue_ref`), it is
adopted. `claude agents --json` — the only data reconcile has to work with — reports
`pid`, `id`, `name` and `state`; it does not report the environment a process was started
with.

That matters because `gh-issues-tui` also stamps every session *it* dispatches with a
provenance environment (`GH_ISSUES_TUI`, `_HARNESS`, `_OWNER`, `_REPO`, ...). In principle
that could distinguish "a session this run dispatched" from "some other `--bg` session
that happens to be named like an issue" — but only by reading it back out of the
*process's* environment after the fact, not from `claude agents --json`.

Issue #148 was filed because the quit confirmation claimed every running session "will be
terminated," which is false for any `bg_dispatch` session — `HarnessRegistry::kill_all`
only ever kills the local `claude attach` viewer, never the background agent. Fixing that
message surfaced a real, separate question: since adoption is name-shaped, a session this
tool did not start (e.g. one from a different `gh-issues-tui` process, or a completely
different tool) can be adopted and then look indistinguishable from one it launched
itself. Killing an adopted session runs `claude stop <bg_id>`, which really does end
someone else's agent.

## Decision

1. Adoption stays name-shaped. No attempt is made to verify that a `--bg` session was
   actually dispatched by this tool.
2. A `/proc/<pid>/environ` check (reading the provenance env back, on Linux, from the
   session's pid) was considered and rejected. Release targets are Linux, macOS
   (`aarch64`/`x86_64`) and Windows; a check that works on one of four targets and
   silently no-ops on the rest is worse than no check, because it would be trusted
   unevenly depending on the platform.
3. Instead, ownership is surfaced rather than enforced: `SessionMeta.adopted: bool`,
   set only by `reconcile`, drawn as `↗` in the session picker and `(adopted)` on the
   identity row. The quit confirmation is grouped by actual fate (`bg_id.is_some()`)
   rather than by adoption — that is what determines whether quitting stops something.
   The kill confirmation is the one place adoption itself is surfaced, because kill is
   the one action that can end a session this tool never started.
4. Nothing is blocked. An adopted session can still be killed; the confirmation just
   says so first.

## Consequences

- No new I/O, no per-platform code path, no silent behavioural difference between
  release targets.
- A user can still kill another harness's agent through gh-issues-tui — same as before
  #148 — but now with the information needed to recognise that before confirming.
- If a future need arises to actually *block* touching a session this tool did not
  start (rather than merely label it), that is a new decision, not a re-litigation of
  this one: it would need a provenance signal this ADR explicitly found no
  cross-platform way to get from `claude agents --json` alone.
