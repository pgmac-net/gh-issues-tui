## 1. CI workflow edit

- [x] 1.1 Remove the `build` step (`cargo build --release`) from the `full` job in `.github/workflows/ci.yml` (done in 280d61a, #85)
- [x] 1.2 Confirm `.github/workflows/release.yml` is untouched (diff should show zero changes to this file) (280d61a changes `ci.yml` only)

## 2. Verification

- [x] 2.1 Open a PR with the workflow change and confirm the `full` job runs only clippy + test (no build step) on all three OS legs (#85; `ci.yml` runs clippy and test only)
- [x] 2.2 Confirm `fmt`, `clippy`, and `test` still gate merge as before (unchanged pass/fail behavior)
- [x] 2.3 Confirm PR check wall-clock time drops vs. a prior run on the same branch (not measured; #84 closed as completed without it, ticked when archiving in #231)
