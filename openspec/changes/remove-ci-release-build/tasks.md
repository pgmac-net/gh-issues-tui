## 1. CI workflow edit

- [ ] 1.1 Remove the `build` step (`cargo build --release`) from the `full` job in `.github/workflows/ci.yml`
- [ ] 1.2 Confirm `.github/workflows/release.yml` is untouched (diff should show zero changes to this file)

## 2. Verification

- [ ] 2.1 Open a PR with the workflow change and confirm the `full` job runs only clippy + test (no build step) on all three OS legs
- [ ] 2.2 Confirm `fmt`, `clippy`, and `test` still gate merge as before (unchanged pass/fail behavior)
- [ ] 2.3 Confirm PR check wall-clock time drops vs. a prior run on the same branch
