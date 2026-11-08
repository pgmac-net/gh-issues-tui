# 14. CI is its own provider, and only completed runs' jobs are cached

## Status

Accepted (#266).

## Context

#266 asks for every workflow run of a repo, grouped and filtered, beside the issue-scoped runs tab of #222. It also asks that the feature can later be pointed at a CI system other than GitHub Actions. Until now the runs calls lived on `IssueProvider` and the domain types were GitHub's JSON shapes, raw `status` and `conclusion` strings included. That ties the tracker to the CI system: a Jira tracker with GitLab CI has no home in that model.

A repo has thousands of runs. Reading a run's jobs costs one REST call, so reading jobs for every listed run would cost 100 calls per page per poll. A completed run never changes; a re-run is a new attempt, and the run list reports it.

## Decision

1. **CI is a separate trait, `ci::CiProvider`.** It owns listing runs, listing workflows, a commit's runs, a run's jobs and ETA history, and its own rate-limit figure. Finding an issue's linked PRs stays on `IssueProvider`: it is a tracker question. GitHub implements both.
2. **The domain is backend-neutral.** Runs, jobs and steps carry a `RunStatus` enum, mapped from the backend's own words at the edge. The estimator, the trees and the renderer read only neutral types. A backend says which filters it can push down (`capabilities()`), and the filter menu offers only those.
3. **The run list is never cached.** It is re-read on every entry to the view and every poll, which is how running and re-run runs stay fresh.
4. **Jobs of completed runs are cached on disk**, keyed by owner, repo, run id and attempt, at most 2000 entries, oldest written first out. The key includes the attempt, so a re-run can never be served the old attempt's jobs. Active runs' jobs are always read fresh. Completed runs' jobs are read only when unfolded.
5. **The repo view polls under ADR 0007's rules**: only while visible with an active run, every 15 s, behind a debounce, held below the REST floor, paused behind a harness session, re-armed only by a landed response. A poll re-reads every loaded page.

## Consequences

- A second CI backend is a new `CiProvider` implementation and a status mapping, not a change to the trees or the estimator.
- The issue tab moved onto the neutral types too; its behaviour is unchanged.
- The cache can only ever be wrong about a run whose attempt number GitHub reuses, which it does not.
- Polling cost grows with loaded pages: one list call per page per poll. `L` is the only thing that adds a page; leaving the repo drops them.
