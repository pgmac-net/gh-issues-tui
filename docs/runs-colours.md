# Runs tab figure colours (#240)

The runs tab drew every figure on the right of a row in one dim colour, so the forecast, the time taken and the step counts were hard to tell apart. They are now three separate colours, each configurable, with defaults that follow existing theme colours. Ticket: [#240](https://github.com/pgmac-net/gh-issues-tui/issues/240). What each colour means: [workflow-runs.md](workflow-runs.md#colours-240).

## What changed

| Figure | Key | Default |
|---|---|---|
| Forecast: time left and ETA clock | `runs_eta`, `runs_eta_bold` | `warning`, bold |
| Time taken | `runs_elapsed` | `dim` (unchanged look) |
| Done/total counts | `runs_progress` | `open` |

Each default is the *resolved* theme colour, so a profile that sets `warning`, `dim` or `open` carries the runs tab along. `runs_eta_bold = false` turns the bold off.

Blocked and overdue use `warning` directly. `?`, `no ETA`, separators, conclusion words and PR-row summaries stay dim.

## How

- `Theme` gains `runs_eta`, `runs_eta_bold`, `runs_elapsed`, `runs_progress`; `ColorProfile` gains the matching optional keys. `with_profile` resolves `warning`, `dim` and `open` first, then the fallbacks.
- A row's right side was one dim `String`. It is now a list of `Seg { fig, text }`. `Fig` is `Dim`, `Warning`, `Eta`, `Elapsed` or `Progress`, and `fig_style` is the only place a figure's style is decided. Separators (` · `) are added between segments, always dim.
- Width and alignment are measured from the flattened text, so the characters on screen, the right-alignment and the name truncation are exactly as before. Only colour and bold change.

## Decisions

- **"ETA" means the whole forecast, not just the clock.** Remaining time and ETA clock are both `Eta`, on run, job, pending-job and PR rows.
- **Problems are not forecasts.** `blocked +1m` and `overdue` keep `warning`.
- **Counts on PR rows stay dim.** The glyph already shows the PR's state, and a green `2/3 failed` would read wrong.
- **The bold is a profile key, not top-level config,** so a `mono` profile can turn it off.
- **The selected row is unchanged:** the highlight is on the name, and the figures keep their colours.

## Verification

- Existing text tests assert the same strings through `flat()`, which joins segments exactly as the old strings were built.
- New tests check each figure's foreground colour and bold on run, job and PR rows, the three fallbacks, and the overrides.
