## Context

`.github/workflows/ci.yml` runs a `full` job matrix (ubuntu/macos/windows) on every PR with three steps: `clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --release`. Investigated whether the `build` step earns its keep:

- `clippy --all-targets` is check-only (MIR-level type-check + lints), no codegen or linking.
- `cargo test` does a full build+link+run at **dev** profile (opt-level 0), so build/link correctness is already exercised on every PR.
- `cargo build --release` repeats that build+link at **release** profile (opt-level 3), on identical source, once per OS leg.
- `Cargo.toml` has no `[profile.release]` section — release profile is pure cargo defaults, no LTO/strip/panic=abort/codegen-units overrides.
- `src/` has no `cfg(debug_assertions)`, `cfg(release)`, or target-gated code — grepped, zero hits.

Given those two facts together, there is currently no code path that can compile successfully at dev profile and fail at release profile. The `build` step is redundant compute, not redundant risk-coverage — but that equivalence is a property of the *current* codebase, not a language guarantee, so it's worth naming as a design decision rather than leaving implicit.

`.github/workflows/release.yml` already runs its own 4-target release build matrix (linux x86_64, macos aarch64, macos x86_64, windows msvc) at tag-push time — including two targets (`macos x86_64` cross-compiled, `linux`) CI's `full` job doesn't even cover today. So CI's build step was never a full stand-in for the release matrix to begin with.

## Goals / Non-Goals

**Goals:**
- Cut PR check wall-clock time by removing a duplicate compile pass.
- Preserve all currently-meaningful PR gates: fmt, clippy (deny warnings), test — unchanged, on all three OS legs.
- Leave `release.yml` untouched — it remains the sole authority on release-buildability.

**Non-Goals:**
- Not attempting to speed up fmt/clippy/test themselves (caching, splitting jobs, etc.) — out of scope for this change.
- Not adding a replacement release-build check anywhere in the PR path (rejected during exploration — see Decisions).
- Not changing `release.yml`'s build matrix, targets, or triggers.

## Decisions

**Decision: remove the `build` step entirely rather than relocate or shrink it.**

Three options were weighed during exploration:
1. **Remove entirely** (chosen) — drop the step from `ci.yml`, no replacement. Lowest cost, matches the current zero-risk read (no profile/cfg divergence exists).
2. Move to a push-to-main-only job — would catch a release-only break right after merge, before a tag is cut, without slowing PR feedback. Rejected: adds a second workflow trigger/job to maintain for a risk that doesn't exist in the codebase today; can be revisited if `[profile.release]` or cfg-gated code is ever introduced.
3. Shrink to a single OS leg (ubuntu only) as a canary — reduces 3x cost to 1x but doesn't eliminate it, and a single-OS canary wouldn't have caught a macOS/Windows-specific release-only break anyway (there are none of those risk categories present regardless of OS). Rejected as complexity without matching benefit.

Option 1 was chosen because the risk it accepts (release-only break reaching `main` undetected until tag time) is currently theoretical, and the mitigation for options 2/3 addresses a risk that doesn't exist yet, at a permanent maintenance cost.

## Risks / Trade-offs

- **[Risk]** A future PR adds a `[profile.release]` override (e.g. `panic = "abort"`, LTO) or `cfg(debug_assertions)`/target-gated code, silently reintroducing the gap this change currently closes for free. → **Mitigation**: none automated; this is an accepted, documented trade-off (see proposal's "Impact" section). If this becomes a real concern later, option 2 above (push-to-main build job) is the identified fallback.
- **[Risk]** `main` can sit release-broken between a merge and the next tag push, since no PR or push-to-main check verifies `cargo build --release` anymore. → **Mitigation**: none added by this change; `release.yml`'s build matrix is still the final gate before any binaries are published, so a broken release blocks the release itself rather than silently shipping — it's a delayed-detection risk, not a shipped-bug risk.

## Migration Plan

Single-file edit to `.github/workflows/ci.yml`: delete the `build` step from the `full` job. No versioning, no rollback tooling needed — revert is `git revert` if the trade-off proves wrong in practice.
