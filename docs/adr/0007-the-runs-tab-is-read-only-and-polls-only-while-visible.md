# 7. The runs tab is read-only, and polls only while visible

## Status

Accepted (#222).

## Context

#222 asks to see the workflow runs associated with an issue, with their jobs, steps, progress and an ETA. Issues have no runs of their own, and everything else the app shows comes from a bulk GraphQL fetch or a single on-demand request. Live progress is different: it means repeated REST calls for as long as a run lasts, and an ETA needs each workflow's history, about 11 requests per workflow.

## Decision

1. **The tab is read-only.** `o` opens GitHub. There is no re-run or cancel, so the feature adds no write path and no confirmation popup.
2. **Polling needs a reason on screen.** A poll fires only while the tab is showing an active run on an open PR. Nothing polls in the background, nothing polls merged PRs, and a full-frame harness session pauses it.
3. **A landed response is the only thing that re-arms a poll.** So fetches for one issue cannot overlap, and a slow API slows the poll instead of stacking requests.
4. **History is session-only and fetched once per workflow and event.** "No history" is cached too: asking again each poll would repeat the whole cost. `r` retries those.
5. **REST keeps its own rate-limit store.** REST and GraphQL are separate budgets, and sharing one store would make the info bar's figure flip between them on every poll. The tab reads the REST store, and holds polling below 100 remaining.

## Consequences

- A PR opened or pushed during a poll appears on `r`, because a poll re-reads known PRs instead of paying for discovery each time.
- Times on screen move every 15 seconds, not every second. Rendering stays a pure function of `App`.
- Adding re-run or cancel later means a new write path with its own confirmation, not a change to this tab's fetches.
