//! Gossip messages already handled, kept far longer than GossipSub's own
//! duplicate cache.
//!
//! GossipSub forgets a message ID after `duplicate_cache_time` (60 s). A node
//! whose event loop has fallen behind, e.g. an archive under load, still holds
//! the messages it has not validated yet: while events are queued its heartbeat
//! does not run, so its message cache does not shift, and it forwards each
//! message when it finally accepts it, minutes late. Receivers whose duplicate
//! cache has expired by then took the copy for a new message, delivered it a
//! second time and forwarded it again. Peers and archives check this set first
//! and ignore such a copy: no delivery, no storage, no forwarding.
//!
//! IDs are those of [`crate::stem::message_id`], the hash GossipSub uses.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// IDs are kept for one to two windows, ten to twenty times GossipSub's
/// duplicate cache. At 150 messages/s that is at most 180,000 IDs, a few MB.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

/// Two generations of IDs: the current window and the previous one.
#[derive(Debug)]
pub struct Seen {
    window: Duration,
    rotate_at: Instant,
    current: HashSet<u64>,
    previous: HashSet<u64>,
}

impl Seen {
    pub fn new(window: Duration, now: Instant) -> Self {
        Seen { window, rotate_at: now + window, current: HashSet::new(), previous: HashSet::new() }
    }

    fn expire(&mut self, now: Instant) {
        if now < self.rotate_at {
            return;
        }
        // After a pause longer than two windows, nothing is still in range.
        self.previous = if now < self.rotate_at + self.window {
            std::mem::take(&mut self.current)
        } else {
            self.current.clear();
            HashSet::new()
        };
        self.rotate_at = now + self.window;
    }

    /// Records `id`; true if it was not seen within the last one to two windows.
    pub fn first_time(&mut self, id: u64, now: Instant) -> bool {
        self.expire(now);
        !self.previous.contains(&id) && self.current.insert(id)
    }
}

static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();

/// This process's set. Do not hold the guard across a call that locks it again.
pub fn seen() -> MutexGuard<'static, Seen> {
    SEEN.get_or_init(|| Mutex::new(Seen::new(WINDOW, Instant::now()))).lock().unwrap()
}

/// Records a received or published gossip message; true the first time.
pub fn first_time(id: u64) -> bool {
    seen().first_time(id, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_secs(600);

    #[test]
    fn a_copy_after_gossipsubs_duplicate_cache_is_a_duplicate() {
        let t0 = Instant::now();
        let mut seen = Seen::new(W, t0);
        assert!(seen.first_time(7, t0));
        assert!(!seen.first_time(7, t0 + Duration::from_secs(61)));
        assert!(seen.first_time(8, t0 + Duration::from_secs(61)));
    }

    #[test]
    fn ids_survive_one_rotation() {
        let t0 = Instant::now();
        let mut seen = Seen::new(W, t0);
        assert!(seen.first_time(7, t0 + W - Duration::from_secs(1)));
        // Rotated once: 7 is in the previous generation.
        assert!(!seen.first_time(7, t0 + W + Duration::from_secs(1)));
    }

    #[test]
    fn ids_expire_after_two_windows() {
        let t0 = Instant::now();
        let mut seen = Seen::new(W, t0);
        assert!(seen.first_time(7, t0));
        assert!(seen.first_time(9, t0 + W)); // first rotation
        assert!(seen.first_time(9 + 1, t0 + 2 * W)); // second rotation drops 7
        assert!(seen.first_time(7, t0 + 2 * W + Duration::from_secs(1)));
    }

    #[test]
    fn a_long_pause_forgets_everything() {
        let t0 = Instant::now();
        let mut seen = Seen::new(W, t0);
        assert!(seen.first_time(7, t0));
        assert!(seen.first_time(7, t0 + 3 * W));
    }
}
