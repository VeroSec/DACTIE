//! Stem phase (Dandelion++) for sharing peers.
//!
//! A sharing peer that publishes an anonymous message hands it to all its
//! GossipSub mesh peers, so a coalition holding one of those slots names the
//! source with first-spy. With the stem, the message first travels along a
//! random path of admitted peers, one hop at a time (`/dactie/stem/1`), and only
//! the peer at its end (a diffuser) publishes it into GossipSub. In the
//! anonymity harness this brought first-spy success down to about the colluding
//! share c/N (docs/DANDELION.md, docs/FUTURE_WORK.md 3.1).
//!
//! Parameters follow Dandelion++ as deployed: two successors per epoch, a
//! diffuser probability q per epoch (Monero 0.2, BIP 156 and Grin 0.1), epochs
//! of 10 minutes plus up to 30 s of jitter, and a fail-safe embargo of 39 s on
//! average (Monero). The anonymity harness has its own copy of the routing
//! logic (`dactie-experiment-anonymity/src/stem.rs`).

use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{request_response, PeerId, StreamProtocol};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::time::{Duration, Instant};

pub const STEM_PROTOCOL: StreamProtocol = StreamProtocol::new("/dactie/stem/1");

/// Largest stem message accepted, as for GossipSub (`max_transmit_size`).
const MAX_STEM_BYTES: usize = 8 * 1024 * 1024;

/// A message on the stem: the signed gossip message exactly as it would be
/// published, and the channel (GossipSub topic) to publish it on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StemMessage {
    pub channel: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StemAck;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StemConfig {
    /// Probability of being a diffuser in an epoch; 0 turns the stem off.
    pub q: f64,
    pub epoch: Duration,
    pub embargo_mean: Duration,
    /// Stem messages accepted per predecessor and second.
    pub rate_per_sec: u32,
}

impl Default for StemConfig {
    fn default() -> Self {
        StemConfig {
            q: 0.2,
            epoch: Duration::from_secs(600),
            embargo_mean: Duration::from_secs(39),
            rate_per_sec: 20,
        }
    }
}

impl StemConfig {
    pub fn is_on(&self) -> bool {
        self.q > 0.0
    }
}

/// What to do with a stem message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Forward(PeerId),
    /// Publish it into GossipSub.
    Fluff,
}

/// The content-addressed ID GossipSub uses for the same bytes.
pub fn message_id(payload: &Vec<u8>) -> u64 {
    let mut h = DefaultHasher::new();
    payload.hash(&mut h);
    h.finish()
}

/// How often the source re-routes its own message after a loop or a failed
/// hop before it leaves the message to the embargo.
const OWN_RETRIES: u8 = 3;

pub struct StemState {
    config: StemConfig,
    epoch_until: Option<Instant>,
    diffuser: bool,
    successors: Vec<PeerId>,
    /// One successor per predecessor for the whole epoch, so repeated messages
    /// of one source take the same path and reveal nothing new.
    routes: HashMap<PeerId, PeerId>,
    /// Successor for this peer's own messages.
    own: Option<PeerId>,
    /// Message IDs are kept for this period and the previous one ([`Self::expire`]);
    /// loops and late stem copies end within seconds.
    rotate_at: Option<Instant>,
    /// Messages seen on the stem.
    seen: HashSet<u64>,
    seen_before: HashSet<u64>,
    /// This peer's own messages, with the re-routes used so far.
    originated: HashMap<u64, u8>,
    originated_before: HashMap<u64, u8>,
    /// Messages known to be in GossipSub ([`Self::in_gossip`]).
    gossiped: HashSet<u64>,
    gossiped_before: HashSet<u64>,
    /// Messages this peer sent on and will publish itself if they do not show
    /// up in GossipSub in time, with their stem predecessor (`None` for its own).
    embargo: HashMap<u64, (Instant, StemMessage, Option<PeerId>)>,
    /// Stem messages per predecessor in the current second.
    rate: HashMap<PeerId, (Instant, u32)>,
}

impl StemState {
    pub fn new(config: StemConfig) -> Self {
        StemState {
            config,
            epoch_until: None,
            diffuser: false,
            successors: Vec::new(),
            routes: HashMap::new(),
            own: None,
            rotate_at: None,
            seen: HashSet::new(),
            seen_before: HashSet::new(),
            originated: HashMap::new(),
            originated_before: HashMap::new(),
            gossiped: HashSet::new(),
            gossiped_before: HashSet::new(),
            embargo: HashMap::new(),
            rate: HashMap::new(),
        }
    }

    pub fn is_on(&self) -> bool {
        self.config.is_on()
    }

    /// Start a new epoch if the current one is over: draw two successors among
    /// the candidates and whether this peer diffuses.
    fn refresh<R: Rng>(&mut self, now: Instant, candidates: &[PeerId], rng: &mut R) {
        if self.epoch_until.is_some_and(|until| now < until) {
            return;
        }
        let jitter = Duration::from_millis(rng.gen_range(0..=30_000));
        self.epoch_until = Some(now + self.config.epoch + jitter);
        self.diffuser = rng.gen_bool(self.config.q.clamp(0.0, 1.0));
        self.successors = candidates.choose_multiple(rng, 2).copied().collect();
        self.routes.clear();
        self.own = self.successors.choose(rng).copied();
    }

    /// Forgets message IDs older than two periods of one stem epoch, so the sets
    /// stay bounded in a long run. Call regularly.
    pub fn expire(&mut self, now: Instant) {
        if self.rotate_at.is_some_and(|at| now < at) {
            return;
        }
        if self.rotate_at.is_some() {
            self.seen_before = std::mem::take(&mut self.seen);
            self.originated_before = std::mem::take(&mut self.originated);
            self.gossiped_before = std::mem::take(&mut self.gossiped);
        }
        self.rotate_at = Some(now + self.config.epoch);
    }

    /// Replace a successor that is no longer a candidate.
    fn repair<R: Rng>(&mut self, gone: PeerId, candidates: &[PeerId], rng: &mut R) -> Option<PeerId> {
        let spare = candidates
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

    /// Where a message goes. `from` is the stem predecessor, `None` for this
    /// peer's own messages, which always enter the stem, even on a diffuser.
    /// `candidates` are the connected, admitted sharing peers.
    pub fn route<R: Rng>(
        &mut self,
        from: Option<PeerId>,
        candidates: &[PeerId],
        now: Instant,
        rng: &mut R,
    ) -> Route {
        self.refresh(now, candidates, rng);
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
            if !candidates.contains(&s) {
                next = self.repair(s, candidates, rng);
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

    /// Whether this is the first time the message reaches this peer on the stem.
    pub fn first_time(&mut self, id: u64) -> bool {
        !self.seen_before.contains(&id) && self.seen.insert(id)
    }

    /// Mark a message as this peer's own.
    pub fn mark_own(&mut self, id: u64) {
        self.originated.entry(id).or_insert(0);
    }

    pub fn is_own(&self, id: u64) -> bool {
        self.originated.contains_key(&id) || self.originated_before.contains_key(&id)
    }

    /// A new successor for one of this peer's own messages that came back on a
    /// loop or whose hop failed. The source must never publish its own message
    /// itself: its mesh peers would receive it straight from the source, which
    /// is exactly what the stem hides. `avoid` holds peers whose hop failed; after
    /// a loop it is empty, so the message goes back to a successor that has seen
    /// it and publishes it as a relay. After `OWN_RETRIES` re-routes, or without
    /// a usable peer, the message is left to the embargo.
    pub fn retry_own<R: Rng>(
        &mut self,
        id: u64,
        avoid: &[PeerId],
        candidates: &[PeerId],
        now: Instant,
        rng: &mut R,
    ) -> Option<PeerId> {
        let used = match self.originated.get_mut(&id) {
            Some(used) => used,
            None => self.originated_before.get_mut(&id)?,
        };
        if *used >= OWN_RETRIES {
            return None;
        }
        *used += 1;
        self.refresh(now, candidates, rng);
        let usable = |p: &PeerId| !avoid.contains(p) && candidates.contains(p);
        let preferred: Vec<PeerId> = self.successors.iter().copied().filter(|p| usable(p)).collect();
        if let Some(p) = preferred.choose(rng) {
            return Some(*p);
        }
        let spare: Vec<PeerId> = candidates.iter().copied().filter(|p| usable(p)).collect();
        spare.choose(rng).copied()
    }

    /// Successor of this peer's own messages in the current epoch.
    pub fn own_successor(&self) -> Option<PeerId> {
        self.own
    }

    /// Whether `from` may send another stem message this second.
    pub fn allow(&mut self, from: PeerId, now: Instant) -> bool {
        let entry = self.rate.entry(from).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(1) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.config.rate_per_sec
    }

    /// Start the fail-safe for a message this peer sent on the stem; `from` is
    /// its stem predecessor, `None` for this peer's own message. Only a peer
    /// that subscribes to the message's channel can see it arrive in GossipSub,
    /// so only such a peer arms an embargo.
    pub fn arm_embargo<R: Rng>(
        &mut self,
        id: u64,
        message: StemMessage,
        from: Option<PeerId>,
        now: Instant,
        rng: &mut R,
    ) {
        // Exponential with the configured mean.
        let u: f64 = rng.gen_range(f64::EPSILON..1.0);
        let wait = self.config.embargo_mean.mul_f64(-u.ln());
        self.embargo.entry(id).or_insert((now + wait, message, from));
    }

    /// The message is in GossipSub, so the stem did its job: this peer saw it
    /// arrive there, or published it itself (GossipSub never hands a peer its
    /// own publication back, so that would otherwise not cancel the timer). Its
    /// embargo ends, and stem copies that still arrive are dropped.
    pub fn in_gossip(&mut self, id: u64) {
        self.embargo.remove(&id);
        self.gossiped.insert(id);
    }

    /// Whether the message is already in GossipSub, see [`Self::in_gossip`]. A
    /// stem copy can still arrive after that if a relay's embargo ran out early
    /// or a hop arrived although its acknowledgement was lost.
    pub fn is_in_gossip(&self, id: u64) -> bool {
        self.gossiped.contains(&id) || self.gossiped_before.contains(&id)
    }

    /// Messages whose embargo ran out, with their stem predecessor: publish them.
    pub fn due(&mut self, now: Instant) -> Vec<(StemMessage, Option<PeerId>)> {
        let ids: Vec<u64> = self
            .embargo
            .iter()
            .filter(|(_, (at, _, _))| *at <= now)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.embargo.remove(&id).map(|(_, message, from)| (message, from)))
            .collect()
    }
}

/// Length-prefixed bincode for stem requests and acknowledgements, so a
/// payload of any size travels as bytes rather than as a JSON number array.
#[derive(Debug, Clone, Default)]
pub struct StemCodec;

async fn read_frame<T: AsyncRead + Unpin + Send>(io: &mut T) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    io.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_STEM_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "stem message too large"));
    }
    let mut buf = vec![0u8; len];
    io.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame<T: AsyncWrite + Unpin + Send>(io: &mut T, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_STEM_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "stem message too large"));
    }
    io.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    io.write_all(bytes).await?;
    io.close().await
}

fn decode<V: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<V> {
    bincode::deserialize(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn encode<V: Serialize>(value: &V) -> io::Result<Vec<u8>> {
    bincode::serialize(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[async_trait]
impl request_response::Codec for StemCodec {
    type Protocol = StreamProtocol;
    type Request = StemMessage;
    type Response = StemAck;

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<StemMessage>
    where
        T: AsyncRead + Unpin + Send,
    {
        decode(&read_frame(io).await?)
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<StemAck>
    where
        T: AsyncRead + Unpin + Send,
    {
        decode(&read_frame(io).await?)
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        req: StemMessage,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &encode(&req)?).await
    }

    async fn write_response<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        res: StemAck,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &encode(&res)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn config(q: f64) -> StemConfig {
        StemConfig { q, ..StemConfig::default() }
    }

    fn peers(n: usize) -> Vec<PeerId> {
        (0..n).map(|_| PeerId::random()).collect()
    }

    /// Within an epoch, each predecessor always gets the same successor, and
    /// own messages always take the same one.
    #[test]
    fn routes_are_fixed_within_an_epoch() {
        let mut rng = StdRng::seed_from_u64(1);
        let candidates = peers(20);
        let mut stem = StemState::new(config(0.0001));
        let now = Instant::now();
        let first_own = stem.route(None, &candidates, now, &mut rng);
        let pred = candidates[0];
        let first = stem.route(Some(pred), &candidates, now, &mut rng);
        for _ in 0..20 {
            assert_eq!(stem.route(None, &candidates, now, &mut rng), first_own);
            assert_eq!(stem.route(Some(pred), &candidates, now, &mut rng), first);
        }
        assert!(matches!(first, Route::Forward(s) if s != pred));
    }

    /// A diffuser publishes relayed messages but still sends its own on the stem.
    #[test]
    fn diffusers_fluff_relayed_but_not_own_messages() {
        let mut rng = StdRng::seed_from_u64(2);
        let candidates = peers(10);
        let mut stem = StemState::new(config(1.0));
        let now = Instant::now();
        assert_eq!(stem.route(Some(candidates[3]), &candidates, now, &mut rng), Route::Fluff);
        assert!(matches!(stem.route(None, &candidates, now, &mut rng), Route::Forward(_)));
    }

    /// Without candidates, a message is published directly.
    #[test]
    fn no_candidates_means_fluff() {
        let mut rng = StdRng::seed_from_u64(3);
        let mut stem = StemState::new(config(0.2));
        assert_eq!(stem.route(None, &[], Instant::now(), &mut rng), Route::Fluff);
    }

    /// The source re-routes its own message away from the peers that failed or
    /// looped, a bounded number of times, and never re-routes others' messages.
    #[test]
    fn own_messages_are_rerouted_not_fluffed() {
        let mut rng = StdRng::seed_from_u64(6);
        let candidates = peers(10);
        let mut stem = StemState::new(config(0.0001));
        let now = Instant::now();
        let Route::Forward(first) = stem.route(None, &candidates, now, &mut rng) else {
            panic!("own messages enter the stem")
        };
        stem.mark_own(42);
        for _ in 0..OWN_RETRIES {
            let next = stem.retry_own(42, &[first], &candidates, now, &mut rng).expect("a spare peer");
            assert_ne!(next, first);
        }
        assert_eq!(stem.retry_own(42, &[first], &candidates, now, &mut rng), None);
        assert_eq!(stem.retry_own(43, &[first], &candidates, now, &mut rng), None);
    }

    /// Each predecessor gets `rate_per_sec` stem messages per second.
    #[test]
    fn predecessors_are_rate_limited() {
        let mut stem = StemState::new(StemConfig { rate_per_sec: 2, ..config(0.2) });
        let (a, b) = (PeerId::random(), PeerId::random());
        let now = Instant::now();
        assert!(stem.allow(a, now) && stem.allow(a, now));
        assert!(!stem.allow(a, now));
        assert!(stem.allow(b, now));
        assert!(stem.allow(a, now + Duration::from_secs(1)));
    }

    /// The embargo releases messages not seen in gossip, and only those, with
    /// the predecessor each came from.
    #[test]
    fn embargo_releases_only_unseen_messages() {
        let mut rng = StdRng::seed_from_u64(5);
        let mut stem = StemState::new(config(0.2));
        let now = Instant::now();
        let from = PeerId::random();
        let m = |p: &[u8]| StemMessage { channel: "channel_1".into(), payload: p.to_vec() };
        stem.arm_embargo(1, m(b"a"), Some(from), now, &mut rng);
        stem.arm_embargo(2, m(b"b"), Some(from), now, &mut rng);
        stem.arm_embargo(3, m(b"c"), None, now, &mut rng);
        stem.in_gossip(1);
        assert!(stem.is_in_gossip(1) && !stem.is_in_gossip(2));
        let later = now + Duration::from_secs(100_000);
        let mut due = stem.due(later);
        due.sort_by(|a, b| a.0.payload.cmp(&b.0.payload));
        assert_eq!(due, vec![(m(b"b"), Some(from)), (m(b"c"), None)]);
        assert!(stem.due(later).is_empty());
    }

    /// Message IDs survive one rotation but not two, so the sets stay bounded
    /// in a long run, also at a peer that never routes a stem message.
    #[test]
    fn ids_expire_after_two_periods() {
        let mut stem = StemState::new(config(0.2));
        let period = stem.config.epoch;
        let now = Instant::now();
        stem.expire(now);
        assert!(stem.first_time(1));
        assert!(!stem.first_time(1));
        stem.in_gossip(2);
        stem.expire(now + period);
        assert!(!stem.first_time(1) && stem.is_in_gossip(2));
        stem.expire(now + period * 2);
        assert!(stem.first_time(1) && !stem.is_in_gossip(2));
    }

    /// Requests and acknowledgements survive the codec, and oversized frames
    /// are refused.
    #[test]
    fn codec_round_trips_and_limits_size() {
        use request_response::Codec;
        futures::executor::block_on(async {
            let mut codec = StemCodec;
            let msg = StemMessage { channel: "channel_9".into(), payload: vec![7; 1000] };
            let mut buf = futures::io::Cursor::new(Vec::new());
            codec.write_request(&STEM_PROTOCOL, &mut buf, msg.clone()).await.unwrap();
            let mut read = futures::io::Cursor::new(buf.into_inner());
            assert_eq!(codec.read_request(&STEM_PROTOCOL, &mut read).await.unwrap(), msg);
            let mut huge = futures::io::Cursor::new(((MAX_STEM_BYTES + 1) as u32).to_be_bytes().to_vec());
            assert!(codec.read_request(&STEM_PROTOCOL, &mut huge).await.is_err());
        });
    }
}
