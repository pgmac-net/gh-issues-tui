//! Pinned issues (#249, ADR 0011): ones this session created or moved, kept
//! in the list until a refetch has seen them.
//!
//! A refetch straight after a write can read a replica that does not have
//! the new issue yet, and a refetch decides membership (ADR 0005), so without
//! a pin the issue would flash in and vanish until the next refresh. A pin is
//! released when a refetch includes the issue, when it has been held for
//! [`PIN_TTL`] (so an issue deleted elsewhere does not stay forever), when a
//! refresh finds the issue gone, or on an org switch.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::App;

/// How long a pin outlives a refetch that cannot see its issue.
pub const PIN_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Default, Clone)]
pub struct Pins {
    at: HashMap<String, Instant>,
}

impl Pins {
    pub fn pin(&mut self, id: String, now: Instant) {
        self.at.insert(id, now);
    }

    pub fn contains(&self, id: &str) -> bool {
        self.at.contains_key(id)
    }

    /// A refetch saw the issue, or a refresh found it gone.
    pub fn release(&mut self, id: &str) {
        self.at.remove(id);
    }

    pub fn clear(&mut self) {
        self.at.clear();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    /// Drop pins held for [`PIN_TTL`] or longer.
    pub fn prune(&mut self, now: Instant) {
        self.at
            .retain(|_, at| now.saturating_duration_since(*at) < PIN_TTL);
    }
}

impl App {
    /// Expire old pins. Called once per event-loop pass, so `app/` never
    /// reads the clock itself.
    pub fn prune_pins(&mut self, now: Instant) {
        self.pins.prune(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_expires_after_the_ttl() {
        let t0 = Instant::now();
        let mut p = Pins::default();
        p.pin("a".into(), t0);
        p.prune(t0 + PIN_TTL - Duration::from_secs(1));
        assert!(p.contains("a"));
        p.prune(t0 + PIN_TTL);
        assert!(!p.contains("a"));
    }

    #[test]
    fn release_and_clear() {
        let t0 = Instant::now();
        let mut p = Pins::default();
        p.pin("a".into(), t0);
        p.pin("b".into(), t0);
        p.release("a");
        assert!(!p.contains("a") && p.contains("b"));
        p.clear();
        assert!(p.is_empty());
    }
}
