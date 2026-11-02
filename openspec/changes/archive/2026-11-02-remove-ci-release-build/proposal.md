## Why

PR checks in `.github/workflows/ci.yml` run a `cargo build --release` step across all three OS legs (ubuntu, macos, windows) on every PR. This is duplicate compile work: `clippy --all-targets` already type-checks the full source tree (check-only, no codegen), and `cargo test` already does a full build+link+run pass at dev profile. The release-profile build+link adds a third, optimized compile of identical source, three times per PR, for no additional signal today — `Cargo.toml` has no `[profile.release]` overrides and `src/` has no `cfg(debug_assertions)`/`cfg(release)`/target-gated code, so a release-only compile break is not currently possible. Removing it should meaningfully cut PR feedback latency.

## What Changes

- Remove the `build` step (`cargo build --release`) from the `full` job in `.github/workflows/ci.yml`.
- Keep `fmt`, `clippy --all-targets -- -D warnings`, and `cargo test` as the PR-blocking checks, unchanged, on all three OS legs.
- `.github/workflows/release.yml` is untouched — its build matrix remains the sole verifier of release-profile builds, running at tag-push time.
- **BREAKING (process, not code)**: release-buildability of `main` is no longer verified on every PR. It's only checked when a tag is pushed. If someone later adds a `[profile.release]` override or `cfg`-gated code, a release-only compile break could land on `main` and go undetected until the next release tag.

## Capabilities

### New Capabilities

- `ci-pr-checks`: What checks are required to pass before a PR can merge, and what's explicitly excluded from that gate.

### Modified Capabilities

(none — no existing specs in `openspec/specs/`)

## Impact

- **Affected file**: `.github/workflows/ci.yml` only.
- **Not affected**: `.github/workflows/release.yml` (must stay exactly as-is — it's the critical release path).
- **CI runtime**: removes one full release-profile compile+link pass (360 transitive deps per `Cargo.lock`) per OS per PR — expected to be the largest single cut in PR check wall-clock time.
- **Risk accepted**: a gap between "release-only break introduced" and "release-only break detected" opens up, bounded by time-to-next-tag rather than time-to-next-PR. Judged low today given no profile/cfg divergence exists, but is a standing tradeoff future changes could erode silently.
