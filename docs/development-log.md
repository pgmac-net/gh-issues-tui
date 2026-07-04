# Development log — initial implementation (2026-07-05)

Work driven by [pgmac-net/homelabia#128](https://github.com/pgmac-net/homelabia/issues/128), phases tracked in sub-issues #129 (repo), #130 (implementation), #131 (docs).

## Process

1. **Repo creation via IaC** — `gh-issues-tui` was added to `config/repos.yaml` in [terraform-github](https://github.com/pgmac-net/terraform-github) (PR #14) under `repos.public`, mirroring the `docker-registry-walk` entry. The PR plan showed exactly 4 resources (repository, branch protection, topics, vulnerability alerts); apply ran automatically on merge to main.
2. **Scaffold** — the empty repo was bootstrapped with a single direct commit to main (README/LICENSE/.gitignore only, unavoidable before a base branch exists); all implementation went through PR #1.
3. **Implementation** — single PR with the full feature set, CI, release workflow, tests and docs.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Language/framework | Rust + ratatui | Matches docker-registry-walk; proven CI + release patterns in the org |
| Fetch strategy | `organization.repositories` → `issues` cursor pagination | GitHub search API caps at 1000 results org-wide |
| Auth | flag → `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token` | Zero-config on machines with `gh`; no stored secrets |
| Mutations | whole-set replacement via `updateIssue` for assignees/labels | One mutation instead of add/remove pairs; input pre-filled with current set |
| Consistency after mutation | full refetch | Simpler than optimistic patching; org fetch is fast enough |
| Closed issues | lazy one-time refetch on first filter switch | Keeps startup fast for the common open-issues case |
| TLS | rustls | No OpenSSL/system deps; leaner CI than docker-registry-walk |

## Diversions from plan

- **README apply instructions were stale**: terraform-github's README describes a manual `apply.yml` workflow, but the live `terraform.yml` applies automatically on push to main. No manual apply step was needed.
- **No `in-progress` label existed** in homelabia; created `status:in-progress` to match the existing `status:*` family rather than inventing a new naming scheme.
- **Initial commit to main**: the never-commit-to-main rule can't apply to an empty repository; one minimal scaffold commit bootstrapped the default branch, everything else via PR.
- **`reqwest` feature rename**: the planned `rustls-tls` feature no longer exists in reqwest 0.13; used `rustls` + `rustls-native-certs` + `http2`.

## Verification

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, 29 unit tests — green locally and in CI (Linux, macOS, Windows).
- Live smoke test against `pgmac-net`: loaded 106 issues across 18 repos; repo grouping rendered with counts.
- Scripted keystroke session (pseudo-tty): searched for a scratch issue, added a comment, closed it via `x`+`y` — both verified with `gh issue view` afterwards; scratch issue deleted.
