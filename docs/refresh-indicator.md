# Refresh indicator (#250)

One character in column 0 of the bottom info bar that spins and changes
colour while issue data is being fetched. Blank when idle, so the bar never
shifts.

## What counts as refreshing

Exactly `App.loading`: the startup fetch, `r` (issue or repo), `Ctrl+R`,
auto-refresh, a widen-refetch and an org switch. Deliberately **not** the
comment-thread fetch (fires on every arrow key), the runs-tab poll (would keep
it on almost permanently while a CI run is active), Jev/semantic search (has
its own `semantic: searching…` text) or the PR summary fetch.

## Glyphs and colour

- Default glyphs: braille `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, **100 ms per frame** (a const,
  `spinner::FRAME`). One revolution takes 1 s.
- Colour is a full-saturation HSV hue from the time since loading began. It
  starts at red and moves smoothly every frame.
- The hue cycle is locked to glyph revolutions: one rainbow cycle =
  `frames × 100 ms × spinner_hue_revolutions`. With the defaults that is
  10 × 100 ms × 10 = **10 s**, so for a typical refresh the colour reads as
  elapsed time. A custom glyph set scales the colour period with it.

## Config

```toml
spinner_glyphs = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"   # frames, one column wide each (default shown)
spinner_hue_revolutions = 10        # glyph revolutions per rainbow cycle, >= 1 (default: 10)
```

A single-frame set (`spinner_glyphs = "●"`) gives a colour-only indicator.

Validation: `spinner_glyphs` is split into grapheme clusters; every frame must be
exactly one terminal column wide and contain no control character (a 2-column
emoji would shift the info bar on alternate frames), and there must be at least
one frame. `spinner_hue_revolutions` must be at least 1.

**A bad value never stops the app starting.** That key falls back to its default
(independently of the other), and the problem is reported twice: on stderr (so it
is in the scrollback after quitting) and as a `⚠` message on the bottom status line
that stays until the first keypress, so the first data load cannot overwrite it.

## How it works

- `tui/spinner.rs`: pure functions of elapsed time (`glyph`, `hue`, `colour`) and
  `Spinner::from_config`, which validates and returns warnings.
- `App.loading_since` is kept in step with `loading` by `App::sync_loading`, called
  once per event-loop pass, so none of the sites that set `loading` know about the
  clock.
- The event loop has a `FRAME` timer branch gated on `App::indicator_animating`
  (loading, and the list is on screen). While idle nothing ticks. Behind an attached
  harness session nothing is drawn, so nothing ticks.
- `ui::list::draw_info_bar` draws column 0; `draw_bottom_line` shows
  `App.config_warning` in preference to the status line while it is set.
