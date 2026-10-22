//! Stem phase (Dandelion++) for the anonymity experiment.
//!
//! Without it, the source hands each message to all its mesh peers, and a
//! coalition holding one of those slots names the source with first-spy. With
//! it, a message first travels along a random path, one peer at a time, and
//! only the peer at its end (a diffuser) publishes it into GossipSub, so the
//! mesh sees the diffuser. Parameters follow Dandelion++ as deployed: two stem
//! successors per epoch, a diffuser probability q per epoch (Monero 0.2, BIP 156
//! and Grin 0.1), epochs of 10 minutes plus up to 30 s of jitter, and a
//! fail-safe embargo of 39 s on average (Monero). See docs/DANDELION.md.

use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use tokio::time::{Duration, Instant};

/// A message on the stem: exactly the bytes that would be published.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StemMessage {
    pub(crate) data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StemAck;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StemConfig {
    /// Probability of being a diffuser in an epoch; 0 turns the stem off.
    pub(crate) q: f64,
    pub(crate) epoch: Duration,
    pub(crate) embargo_mean: Duration,
}

impl StemConfig {
    pub(crate) const OFF: StemConfig = StemConfig {
        q: 0.0,
        epoch: Duration::from_secs(600),
        embargo_mean: Duration::from_secs(39),
    };

    pub(crate) fn is_on(&self) -> bool {
        self.q > 0.0
    }
}

/// What to do with a stem message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Forward(PeerId),
    /// Publish it into GossipSub.
    Fluff,
}

/// The content-addressed ID GossipSub uses for the same bytes.
pub(crate) fn message_id(data: &Vec<u8>) -> u64 {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

pub(crate) struct StemState {
    config: StemConfig,
    epoch_until: Option<Instant>,
    diffuser: bool,
    successors: Vec<PeerId>,
    /// One successor per predecessor for the whole epoch, so repeated messages
    /// of one source take the same path and reveal nothing new.
    routes: HashMap<PeerId, PeerId>,
    /// Successor for this peer's own messages.
    own: Option<PeerId>,
    seen: HashSet<u64>,
    embargo: HashMap<u64, (Instant, Vec<u8>)>,
    /// This peer's own messages, with the re-routes used so far.
    originated: HashMap<u64, u8>,
}

/// How often the source re-routes its own message after a loop or a failed
/// hop before it leaves the message to the embargo.
const OWN_RETRIES: u8 = 3;

impl StemState {
    pub(crate) fn new(config: StemConfig) -> Self {
        StemState {
            config,
            epoch_until: None,
            diffuser: false,
            successors: Vec::new(),
            routes: HashMap::new(),
            own: None,
            seen: HashSet::new(),
            embargo: HashMap::new(),
            originated: HashMap::new(),
        }
    }

    pub(crate) fn is_on(&self) -> bool {
        self.config.is_on()
    }

    /// Start a new epoch if the current one is over: draw two successors among
    /// the connected peers and whether this peer diffuses.
    fn refresh<R: Rng>(&mut self, now: Instant, connected: &[PeerId], rng: &mut R) {
        if self.epoch_until.is_some_and(|until| now < until) {
            self.top_up(connected, rng);
            return;
        }
        let jitter = Duration::from_millis(rng.gen_range(0..=30_000));
        self.epoch_until = Some(now + self.config.epoch + jitter);
        self.diffuser = rng.gen_bool(self.config.q.clamp(0.0, 1.0));
        self.successors = connected.choose_multiple(rng, 2).copied().collect();
        self.routes.clear();
        self.own = self.successors.choose(rng).copied();
    }

    /// Adds successors while the epoch has fewer than two, e.g. because the peer
    /// published background traffic before its first connection: without this,
    /// its own messages, the measurement message included, would skip the stem
    /// for the rest of the epoch (15 of the 240 runs in the first stem grid).
    fn top_up<R: Rng>(&mut self, connected: &[PeerId], rng: &mut R) {
        while self.successors.len() < 2 {
            let spare: Vec<PeerId> =
                connected.iter().copied().filter(|p| !self.successors.contains(p)).collect();
            let Some(p) = spare.choose(rng).copied() else { break };
            self.successors.push(p);
        }
        if self.own.is_none() {
            self.own = self.successors.choose(rng).copied();
        }
    }

    /// Replace a successor that is no longer connected.
    fn repair<R: Rng>(&mut self, gone: PeerId, connected: &[PeerId], rng: &mut R) -> Option<PeerId> {
        let spare = connected
            .iter()
            .copied()
            .filter(|p| !self.successors.contains(p))
            .collect::<Vec<_>>()
            .choose(rng)
            .copied();
        self.successors.retain(|p| *p != gone);
        self.successors.extend(spare);
        self.routes.retain(|_, s| *s != gone);
        if self.own == Some(gone) {
            self.own = spare;
        }
        spare
    }

    /// Where a message goes: `from` is the stem predecessor, `None` for this
    /// peer's own messages, which always enter the stem, even on a diffuser.
    pub(crate) fn route<R: Rng>(
        &mut self,
        from: Option<PeerId>,
        connected: &[PeerId],
        now: Instant,
        rng: &mut R,
    ) -> Route {
        self.refresh(now, connected, rng);
        if from.is_some() && self.diffuser {
            return Route::Fluff;
        }
        let mut next = match from {
            None => self.own,
            Some(p) => match self.routes.get(&p) {
                Some(s) => Some(*s),
                None => {
                    // Prefer a successor other than the predecessor, so the
                    // message does not bounce straight back.
                    let options: Vec<PeerId> =
                        self.successors.iter().copied().filter(|s| *s != p).collect();
                    let s = options.choose(rng).copied();
                    if let Some(s) = s {
                        self.routes.insert(p, s);
                    }
                    s
                }
            },
        };
        if let Some(s) = next {
            if !connected.contains(&s) {
                next = self.repair(s, connected, rng);
                if let (Some(p), Some(n)) = (from, next) {
                    self.routes.insert(p, n);
                }
            }
        }
        match next {
            Some(s) if Some(s) != from => Route::Forward(s),
            _ => Route::Fluff,
        }
    }

    /// Mark a message as this peer's own.
    pub(crate) fn mark_own(&mut self, id: u64) {
        self.originated.entry(id).or_insert(0);
    }

    pub(crate) fn is_own(&self, id: u64) -> bool {
        self.originated.contains_key(&id)
    }

    /// A new successor for one of this peer's own messages that came back on a
    /// loop or whose hop failed. The source must never fluff its own message
    /// itself: its mesh peers would then receive it straight from the source,
    /// which is exactly what the stem hides. `avoid` holds the peers that led
    /// into the loop or failed. After `OWN_RETRIES` re-routes, or without a
    /// spare peer, the message is left to the embargo.
    pub(crate) fn retry_own<R: Rng>(
        &mut self,
        id: u64,
        avoid: &[PeerId],
        connected: &[PeerId],
        now: Instant,
        rng: &mut R,
    ) -> Option<PeerId> {
        let used = self.originated.get_mut(&id)?;
        if *used >= OWN_RETRIES {
            return None;
        }
        *used += 1;
        self.refresh(now, connected, rng);
        let usable = |p: &PeerId| !avoid.contains(p) && connected.contains(p);
        let preferred: Vec<PeerId> = self.successors.iter().copied().filter(|p| usable(p)).collect();
        if let Some(p) = preferred.choose(rng) {
            return Some(*p);
        }
        let spare: Vec<PeerId> = connected.iter().copied().filter(|p| usable(p)).collect();
        spare.choose(rng).copied()
    }

    /// Successor of this peer's own messages in the current epoch.
    pub(crate) fn own_successor(&self) -> Option<PeerId> {
        self.own
    }

    /// Whether this is the first time the message reaches this peer on the stem.
    /// A repeat means the stem looped; the caller then fluffs at once instead of
    /// waiting for the embargo.
    pub(crate) fn first_time(&mut self, id: u64) -> bool {
        self.seen.insert(id)
    }

    /// Start the fail-safe for a message this peer forwarded on the stem.
    pub(crate) fn arm_embargo<R: Rng>(&mut self, id: u64, data: Vec<u8>, now: Instant, rng: &mut R) {
        // Exponential with the configured mean.
        let u: f64 = rng.gen_range(f64::EPSILON..1.0);
        let wait = self.config.embargo_mean.mul_f64(-u.ln());
        self.embargo.entry(id).or_insert((now + wait, data));
    }

    /// The message arrived through GossipSub, so the stem did its job.
    pub(crate) fn seen_in_gossip(&mut self, id: u64) {
        self.embargo.remove(&id);
    }

    /// Messages whose embargo ran out: publish them.
    pub(crate) fn due(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let ids: Vec<u64> = self
            .embargo
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.embargo.remove(&id).map(|(_, data)| data))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn config(q: f64) -> StemConfig {
        StemConfig { q, ..StemConfig::OFF }
    }

    fn peers(n: usize) -> Vec<PeerId> {
        (0..n).map(|_| PeerId::random()).collect()
    }

    /// An epoch that began before the first connection gets successors as soon
    /// as there are connections, instead of skipping the stem until it ends.
    #[test]
    fn an_empty_epoch_is_topped_up() {
        let mut rng = StdRng::seed_from_u64(10);
        let mut stem = StemState::new(config(0.0001));
        let now = Instant::now();
        assert_eq!(stem.route(None, &[], now, &mut rng), Route::Fluff);
        let connected = peers(5);
        assert!(matches!(stem.route(None, &connected, now, &mut rng), Route::Forward(_)));
    }

    /// Within an epoch, each predecessor always gets the same successor, and
    /// own messages always take the same one.
    #[test]
    fn routes_are_fixed_within_an_epoch() {
        let mut rng = StdRng::seed_from_u64(1);
        let connected = peers(20);
        let mut stem = StemState::new(config(0.0001));
        let now = Instant::now();
        let first_own = stem.route(None, &connected, now, &mut rng);
        let pred = connected[0];
        let first = stem.route(Some(pred), &connected, now, &mut rng);
        for _ in 0..20 {
            assert_eq!(stem.route(None, &connected, now, &mut rng), first_own);
            assert_eq!(stem.route(Some(pred), &connected, now, &mut rng), first);
        }
        assert!(matches!(first, Route::Forward(s) if s != pred));
    }

    /// A diffuser fluffs relayed messages but still sends its own on the stem.
    #[test]
    fn diffusers_fluff_relayed_but_not_own_messages() {
        let mut rng = StdRng::seed_from_u64(2);
        let connected = peers(10);
        let mut stem = StemState::new(config(1.0));
        let now = Instant::now();
        assert_eq!(stem.route(Some(connected[3]), &connected, now, &mut rng), Route::Fluff);
        assert!(matches!(stem.route(None, &connected, now, &mut rng), Route::Forward(_)));
    }

    /// Over many epochs, the share of diffusers matches q.
    #[test]
    fn diffuser_share_matches_q() {
        let mut rng = StdRng::seed_from_u64(3);
        let connected = peers(10);
        let mut fluffs = 0;
        let trials = 4000;
        for _ in 0..trials {
            let mut stem = StemState::new(config(0.2));
            if stem.route(Some(connected[0]), &connected, Instant::now(), &mut rng) == Route::Fluff {
                fluffs += 1;
            }
        }
        let share = fluffs as f64 / trials as f64;
        assert!((0.17..0.23).contains(&share), "diffuser share {share}");
    }

    /// A successor that disconnected is replaced by another connected peer.
    #[test]
    fn disconnected_successors_are_replaced() {
        let mut rng = StdRng::seed_from_u64(4);
        let connected = peers(10);
        let mut stem = StemState::new(config(0.0001));
        let now = Instant::now();
        let Route::Forward(s) = stem.route(None, &connected, now, &mut rng) else {
            panic!("own messages enter the stem")
        };
        let remaining: Vec<PeerId> = connected.iter().copied().filter(|p| *p != s).collect();
        let Route::Forward(t) = stem.route(None, &remaining, now, &mut rng) else {
            panic!("a spare successor exists")
        };
        assert_ne!(s, t);
        assert!(remaining.contains(&t));
    }

    /// The source re-routes its own message away from the peers that failed or
    /// looped, a bounded number of times, and never re-routes others' messages.
    #[test]
    fn own_messages_are_rerouted_not_fluffed() {
        let mut rng = StdRng::seed_from_u64(6);
        let connected = peers(10);
        let mut stem = StemState::new(config(0.0001));
        let now = Instant::now();
        let Route::Forward(first) = stem.route(None, &connected, now, &mut rng) else {
            panic!("own messages enter the stem")
        };
        stem.mark_own(42);
        for _ in 0..OWN_RETRIES {
            let next = stem.retry_own(42, &[first], &connected, now, &mut rng).expect("a spare peer");
            assert_ne!(next, first);
        }
        assert_eq!(stem.retry_own(42, &[first], &connected, now, &mut rng), None);
        assert_eq!(stem.retry_own(43, &[first], &connected, now, &mut rng), None);
    }

    /// The embargo releases messages not seen in gossip, and only those.
    #[test]
    fn embargo_releases_only_unseen_messages() {
        let mut rng = StdRng::seed_from_u64(5);
        let mut stem = StemState::new(config(0.2));
        let now = Instant::now();
        stem.arm_embargo(1, b"a".to_vec(), now, &mut rng);
        stem.arm_embargo(2, b"b".to_vec(), now, &mut rng);
        stem.seen_in_gossip(1);
        let later = now + Duration::from_secs(10_000);
        assert_eq!(stem.due(later), vec![b"b".to_vec()]);
        assert!(stem.due(later).is_empty());
        assert!(stem.first_time(7));
        assert!(!stem.first_time(7));
    }
}
