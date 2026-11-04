//! The refresh indicator (#250): one character in column 0 of the info bar
//! that spins and changes colour while issue data is being fetched.
//!
//! Pure functions of elapsed time, so the renderer needs only a clock and the
//! tests need none. The hue is locked to glyph revolutions: one rainbow cycle
//! is `revolutions` trips round the frame set, so a custom glyph set scales
//! the colour period with it instead of fighting it.

use std::time::Duration;

use ratatui::style::Color;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How long each glyph is shown. Fixed: 10 fps is smooth without being busy.
pub const FRAME: Duration = Duration::from_millis(100);

/// Braille spinner. Every braille cell is one column wide in any font that has it.
pub const DEFAULT_GLYPHS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

/// With the default glyphs this is a 10 s rainbow cycle.
pub const DEFAULT_REVOLUTIONS: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spinner {
    frames: Vec<String>,
    revolutions: u32,
}

impl Default for Spinner {
    fn default() -> Self {
        Self {
            frames: split_default(),
            revolutions: DEFAULT_REVOLUTIONS,
        }
    }
}

fn split_default() -> Vec<String> {
    DEFAULT_GLYPHS.graphemes(true).map(String::from).collect()
}

impl Spinner {
    /// Build from the two config keys. A bad value never errors: it is
    /// replaced by its default and named in the returned warnings, so a typo
    /// in a cosmetic setting cannot stop the app starting.
    pub fn from_config(glyphs: Option<&str>, revolutions: Option<i64>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut spinner = Self::default();

        if let Some(g) = glyphs {
            match parse_glyphs(g) {
                Ok(frames) => spinner.frames = frames,
                Err(why) => warnings.push(format!("spinner_glyphs: {why} — using default")),
            }
        }
        if let Some(r) = revolutions {
            match u32::try_from(r) {
                Ok(n) if n >= 1 => spinner.revolutions = n,
                _ => warnings.push(format!(
                    "spinner_hue_revolutions: {r} is not at least 1 — using default"
                )),
            }
        }
        (spinner, warnings)
    }

    fn frame_ms(&self) -> u128 {
        FRAME.as_millis()
    }

    /// Length of one full rainbow cycle.
    pub fn period(&self) -> Duration {
        FRAME * self.frames.len() as u32 * self.revolutions
    }

    /// The glyph to show `elapsed` after loading began.
    pub fn glyph(&self, elapsed: Duration) -> &str {
        let idx = (elapsed.as_millis() / self.frame_ms()) % self.frames.len() as u128;
        &self.frames[idx as usize]
    }

    /// Hue in degrees, `0.0..360.0`; red at the start of every cycle.
    pub fn hue(&self, elapsed: Duration) -> f64 {
        let period = self.period().as_millis();
        let into = elapsed.as_millis() % period;
        into as f64 / period as f64 * 360.0
    }

    /// Full-saturation rainbow colour for `elapsed`.
    pub fn colour(&self, elapsed: Duration) -> Color {
        hue_to_rgb(self.hue(elapsed))
    }
}

/// Split into grapheme clusters; each must be exactly one column wide, or the
/// info bar would shift on alternate frames.
fn parse_glyphs(s: &str) -> Result<Vec<String>, String> {
    let frames: Vec<String> = s.graphemes(true).map(String::from).collect();
    if frames.is_empty() {
        return Err("is empty".into());
    }
    // `width` counts a control character as a column, but writing one to the
    // terminal would act on it instead of drawing it.
    if frames.iter().any(|f| f.chars().any(char::is_control)) {
        return Err("contains a control character".into());
    }
    if let Some(bad) = frames.iter().find(|f| f.width() != 1) {
        return Err(format!(
            "frame {bad:?} is {} columns wide, not 1",
            bad.width()
        ));
    }
    Ok(frames)
}

/// HSV with saturation and value 1, hue in degrees.
fn hue_to_rgb(hue: f64) -> Color {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let byte = |v: f64| (v * 255.0).round() as u8;
    Color::Rgb(byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn default_is_ten_braille_frames_and_a_ten_second_cycle() {
        let s = Spinner::default();
        assert_eq!(s.frames.len(), 10);
        assert_eq!(s.period(), Duration::from_secs(10));
    }

    #[test]
    fn each_glyph_lasts_one_frame_and_the_set_wraps() {
        let s = Spinner::default();
        assert_eq!(s.glyph(ms(0)), "⠋");
        assert_eq!(s.glyph(ms(99)), "⠋");
        assert_eq!(s.glyph(ms(100)), "⠙");
        assert_eq!(s.glyph(ms(1000)), "⠋", "one revolution per second");
    }

    #[test]
    fn hue_starts_red_and_wraps_at_the_period() {
        let s = Spinner::default();
        assert_eq!(s.colour(ms(0)), Color::Rgb(255, 0, 0));
        assert_eq!(s.colour(ms(10_000)), Color::Rgb(255, 0, 0));
        assert_eq!(s.colour(ms(5_000)), Color::Rgb(0, 255, 255), "180° is cyan");
    }

    #[test]
    fn hue_moves_every_frame() {
        let s = Spinner::default();
        assert_ne!(s.colour(ms(0)), s.colour(ms(100)));
    }

    #[test]
    fn primary_hues() {
        assert_eq!(hue_to_rgb(0.0), Color::Rgb(255, 0, 0));
        assert_eq!(hue_to_rgb(60.0), Color::Rgb(255, 255, 0));
        assert_eq!(hue_to_rgb(120.0), Color::Rgb(0, 255, 0));
        assert_eq!(hue_to_rgb(240.0), Color::Rgb(0, 0, 255));
        assert_eq!(hue_to_rgb(300.0), Color::Rgb(255, 0, 255));
    }

    #[test]
    fn the_period_scales_with_frames_and_revolutions() {
        let (s, w) = Spinner::from_config(Some("/-\\|"), Some(5));
        assert!(w.is_empty());
        assert_eq!(s.period(), ms(100 * 4 * 5));
        assert_eq!(s.glyph(ms(200)), "\\");
    }

    #[test]
    fn a_single_frame_is_a_colour_only_indicator() {
        let (s, w) = Spinner::from_config(Some("●"), None);
        assert!(w.is_empty());
        assert_eq!(s.glyph(ms(12_345)), "●");
        assert_eq!(s.period(), Duration::from_secs(1));
    }

    #[test]
    fn a_combining_sequence_is_one_frame_when_one_column() {
        // 'e' + combining acute: two chars, one grapheme, one column.
        let (s, w) = Spinner::from_config(Some("e\u{301}x"), None);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.frames.len(), 2);
    }

    #[test]
    fn bad_glyphs_warn_and_fall_back() {
        for bad in ["", "🔄", "a🔄b", "\u{0}"] {
            let (s, w) = Spinner::from_config(Some(bad), None);
            assert_eq!(s, Spinner::default(), "{bad:?}");
            assert_eq!(w.len(), 1, "{bad:?}");
            assert!(w[0].starts_with("spinner_glyphs:"), "{}", w[0]);
            assert!(w[0].ends_with("using default"), "{}", w[0]);
        }
    }

    #[test]
    fn bad_revolutions_warn_and_fall_back() {
        for bad in [0, -3, i64::MAX] {
            let (s, w) = Spinner::from_config(None, Some(bad));
            assert_eq!(s, Spinner::default(), "{bad}");
            assert_eq!(w.len(), 1, "{bad}");
            assert!(w[0].starts_with("spinner_hue_revolutions:"), "{}", w[0]);
        }
    }

    #[test]
    fn each_key_falls_back_independently() {
        let (s, w) = Spinner::from_config(Some(""), Some(3));
        assert_eq!(w.len(), 1);
        assert_eq!(s.frames.len(), 10);
        assert_eq!(s.revolutions, 3);
    }
}
