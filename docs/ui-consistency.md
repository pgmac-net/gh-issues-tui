# UI interaction consistency (#233)

Part 5, the last part, of #227. A survey ([#233 comment](https://github.com/pgmac-net/gh-issues-tui/issues/233#issuecomment-6575799837)) checked three things:
- every Normal-mode key against the in-app help and the README
- which keys each of the 23 mode handlers accepts
- all 59 status-line messages

This page records what was made consistent, and what was already consistent on purpose.

## Changed

| What | Before | After |
|---|---|---|
| `Z` in the `?` keys page | missing (README only) | listed in `LIST_HELP` |
| Harness and session pickers | private `picker_nav`: no wrap, no `Home`/`End`, no `Ctrl+U`, Ctrl-chords typed | `keys::shared::picker_common_key`, like the other 13 pickers; Esc still returns to a running session |
| Yes/No popups (close/reopen, move, inferred priority, harness quit/kill) | four copies of the key `match`; the first three took `h`/`l` but not `Y`/`N`, the harness one the reverse | one `keys::shared::confirm_decision`: `y`/`Y`, `n`/`N`, `Esc`, `Enter`, `←`/`→`/`h`/`l`/`Tab`/`Shift+Tab` everywhere. Each popup still decides what Yes and No do, and defaults to No (ADR 0003 unchanged) |
| A failed mutation | `failed: {e}` | `update failed: {e}`, naming its subject like every other failure |

## Already consistent, on purpose

- **Esc** closes every modal. The full-frame harness session is the exception by design: keys go to the agent, and `F12` escapes.
- **Type-ahead pickers** take arrows but not `j`/`k`/`q`, because letters are typed into the filter. **Read-only popups** (filter menu, calendar, help, PR summary) take both and close on `q`.
- **In-progress messages** all use `…`, never `...`.
- **"Not supported" messages** read `<feature> not supported by this provider`.
- **Rate-limit failures** in refreshes read `failed — {e}` (dash), a deliberate marker against `failed: {e}`.

## For new code

A new picker uses `picker_common_key`, and a new confirm popup uses `confirm_decision`. Neither writes its own key `match`. CLAUDE.md records this as an invariant.
