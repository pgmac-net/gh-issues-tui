## ADDED Requirements

### Requirement: PR-blocking checks
The system SHALL run `fmt`, `clippy --all-targets -- -D warnings`, and `cargo test` on every pull request across the `full` job's OS matrix (ubuntu, macos, windows), and these SHALL be the checks required to pass before merge.

#### Scenario: PR opened or updated
- **WHEN** a pull request is opened or receives a new commit
- **THEN** `.github/workflows/ci.yml` runs `fmt` (ubuntu), and `clippy` + `test` on each OS in the `full` matrix

#### Scenario: A PR check fails
- **WHEN** `fmt`, `clippy`, or `test` fails on a required OS leg (ubuntu/macos required; windows is `allow_failure`)
- **THEN** the PR is blocked from merging until it passes

### Requirement: PR checks exclude release-profile build
The PR-blocking checks SHALL NOT include a `cargo build --release` step. Release-profile buildability is verified only by `.github/workflows/release.yml` at tag-push time, not on every PR.

#### Scenario: PR check run completes
- **WHEN** the `full` job finishes on a pull request
- **THEN** no release-profile (`cargo build --release`) compilation has occurred as part of that run

#### Scenario: Tag is pushed
- **WHEN** a `v*` tag is pushed
- **THEN** `.github/workflows/release.yml`'s build matrix performs the release-profile build across all 4 release targets, independent of and unaffected by this change
