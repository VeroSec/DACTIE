//! Topology-aware timing attack on source anonymity.
//!
//! The estimators in `adversary` use only what the colluders see: who relayed
//! the message to them and when. This module adds the worst-case insider, a
//! coalition that also knows the propagation graph at the measurement (every
//! peer's GossipSub mesh) and the true delay of every link. Fanti and Viswanath
//! (NeurIPS 2017) give maximum-likelihood estimators for a supernode connected
//! to every peer; for a coalition of insiders, their spy-based adversary, they
//! point to heuristics such as Pinto, Thiran and Vetterli (2012), which match
//! the spies' receipt times against path lengths from each candidate source.
//! The estimators here do that:
//!
//! - `timing_delay`: a message from s reaches colluder j at t0 + d(s, j), with d
//!   the fastest path over the mesh, each hop costing its link delay plus a
//!   processing constant. The publish time t0 is unknown to the coalition and
//!   fitted as the mean residual; the guess is the candidate with the smallest
//!   sum of squared residuals.
//! - `timing_hops`: the same with fewest hops times a hop time, for a coalition
//!   that knows the topology but not the delays.
//! - `timing_delay_stem`: with the stem on, the message enters the mesh at the
//!   diffuser, so the two estimators above find the diffuser, not the source.
//!   This variant applies the stem-aware first-spy rule when a colluder sat on
//!   the stem and `timing_delay` otherwise.
//!
//! Only GossipSub receipts enter the fit, each colluder's earliest: stem hops
//! do not follow the mesh. Shortest paths are computed once per run from every
//! peer, so a sampled coalition costs O(N c).

use crate::adversary::{self, Estimator, Observation};
use crate::link_latency::LatencyRange;
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use rand::seq::SliceRandom;
use rand::Rng;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::time::Duration;

/// The propagation graph at the measurement. GossipSub forwards a message to
/// the mesh peers of every peer it reaches, and with flood publishing off the
/// source sends it to its own mesh only, so messages travel the mesh links.
#[derive(Debug, Clone, Default)]
pub(crate) struct Topology {
    /// Each peer's mesh peers, with the one-way delay of the link to them.
    pub(crate) links: HashMap<PeerId, Vec<(PeerId, Duration)>>,
}

impl Topology {
    /// From the listening addresses and meshes the peers reported. A link's
    /// delay follows from the listening ports of its ends, as in the transport
    /// (`LatencyRange::for_ports`); a peer whose port is unknown gets no links.
    pub(crate) fn from_meshes(
        addrs: &[(PeerId, Multiaddr)],
        meshes: &HashMap<PeerId, Vec<PeerId>>,
        latency: LatencyRange,
    ) -> Self {
        let ports: HashMap<PeerId, u16> = addrs
            .iter()
            .filter_map(|(peer, addr)| {
                addr.iter().find_map(|p| match p {
                    Protocol::Tcp(port) => Some((*peer, port)),
                    _ => None,
                })
            })
            .collect();
        let links = meshes
            .iter()
            .filter_map(|(peer, mesh)| {
                let from = *ports.get(peer)?;
                let out = mesh
                    .iter()
                    .filter_map(|m| Some((*m, latency.for_ports(from, *ports.get(m)?))))
                    .collect();
                Some((*peer, out))
            })
            .collect();
        Topology { links }
    }

    fn delay(&self, from: &PeerId, to: &PeerId) -> Option<Duration> {
        self.links.get(from)?.iter().find(|(p, _)| p == to).map(|(_, d)| *d)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TimingEstimator {
    /// Fit the colluders' receipt times to fastest-path delays from each
    /// candidate, the start time unknown.
    Delay,
    /// The same with hop counts: the topology is known, the delays are not.
    Hops,
    /// Stem-aware first-spy if a colluder received the message on the stem,
    /// `Delay` otherwise.
    DelayStem,
}

impl TimingEstimator {
    pub(crate) const ALL: [TimingEstimator; 3] = [
        TimingEstimator::Delay,
        TimingEstimator::Hops,
        TimingEstimator::DelayStem,
    ];
}

impl fmt::Display for TimingEstimator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TimingEstimator::Delay => "timing_delay",
            TimingEstimator::Hops => "timing_hops",
            TimingEstimator::DelayStem => "timing_delay_stem",
        };
        write!(f, "{s}")
    }
}

/// Sums of squared residuals closer than this (in µs²) are ties. It only
/// absorbs rounding: candidates whose predictions differ by a constant fit
/// equally well, but summing in a different order leaves them a few ulps apart.
const TIE_US2: f64 = 1e-3;

/// What a topology-aware coalition precomputes once per run.
pub(crate) struct TimingModel {
    /// The run's peers; the matrices are indexed by position here.
    peers: Vec<PeerId>,
    /// Fastest-path time in µs from peer s (row) to peer j (column) over the
    /// mesh, every hop costing its link delay plus `processing_us`;
    /// `u64::MAX` where j cannot be reached.
    delay_us: Vec<u64>,
    /// Fewest mesh hops from s to j; `u32::MAX` where j cannot be reached.
    hops: Vec<u32>,
    /// Each peer's earliest GossipSub receipt, in µs after the publish.
    first_gossip: Vec<Option<f64>>,
    /// Processing per hop in µs, on top of the link delay.
    pub(crate) processing_us: f64,
    /// Time per hop in µs, link delay included, for `timing_hops`.
    pub(crate) hop_us: f64,
    /// Relay steps the two constants were estimated from.
    pub(crate) relay_steps: usize,
}

impl TimingModel {
    /// The model with its per-hop constants estimated from the run's trace.
    ///
    /// A peer j whose first copy came from i received it one link delay plus
    /// i's processing after i did. The median of t_j - t_i - w(i, j) over all
    /// such relay steps is the processing constant, the median of t_j - t_i the
    /// hop time. Neither involves the source or the coalition: both are
    /// properties of the network, which a coalition that knows the topology
    /// can measure on its own messages.
    pub(crate) fn calibrated(
        topology: &Topology,
        peers: &[PeerId],
        observations: &[Observation],
    ) -> Self {
        let first = first_gossip_receipts(observations);
        let mut processing = Vec::new();
        let mut hop = Vec::new();
        for (j, (t_j, i)) in &first {
            let (Some((t_i, _)), Some(w)) = (first.get(i), topology.delay(i, j)) else {
                continue;
            };
            processing.push(t_j - t_i - w.as_secs_f64() * 1e6);
            hop.push(t_j - t_i);
        }
        let relay_steps = hop.len();
        // Processing cannot be negative; a scheduler that stamps a receipt
        // late and the next one early can make single steps look so.
        let processing_us = median(&mut processing).unwrap_or(0.0).max(0.0);
        // Without relay steps (tiny networks) a hop costs its link delay.
        let hop_us = median(&mut hop).unwrap_or_else(|| mean_link_us(topology));
        let mut model = Self::new(topology, peers, observations, processing_us, hop_us);
        model.relay_steps = relay_steps;
        model
    }

    /// The model with given per-hop constants: shortest paths from every peer.
    pub(crate) fn new(
        topology: &Topology,
        peers: &[PeerId],
        observations: &[Observation],
        processing_us: f64,
        hop_us: f64,
    ) -> Self {
        let n = peers.len();
        let index: HashMap<PeerId, usize> = peers.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let step = processing_us.round() as u64;
        let adjacency: Vec<Vec<(usize, u64)>> = peers
            .iter()
            .map(|p| {
                topology
                    .links
                    .get(p)
                    .into_iter()
                    .flatten()
                    .filter_map(|(m, d)| Some((*index.get(m)?, d.as_micros() as u64 + step)))
                    .collect()
            })
            .collect();
        let mut delay_us = Vec::with_capacity(n * n);
        let mut hops = Vec::with_capacity(n * n);
        for s in 0..n {
            delay_us.extend(dijkstra(&adjacency, s));
            hops.extend(bfs(&adjacency, s));
        }
        let mut first_gossip = vec![None; n];
        for (peer, (t, _)) in first_gossip_receipts(observations) {
            if let Some(i) = index.get(&peer) {
                first_gossip[*i] = Some(t);
            }
        }
        TimingModel {
            peers: peers.to_vec(),
            delay_us,
            hops,
            first_gossip,
            processing_us,
            hop_us,
            relay_steps: 0,
        }
    }

    fn predicted_delay(&self, s: usize, j: usize) -> Option<f64> {
        let d = self.delay_us[s * self.peers.len() + j];
        (d != u64::MAX).then_some(d as f64)
    }

    fn predicted_hops(&self, s: usize, j: usize) -> Option<f64> {
        let h = self.hops[s * self.peers.len() + j];
        (h != u32::MAX).then_some(h as f64 * self.hop_us)
    }

    /// How well the delay model explains the trace from the true source: the
    /// RMS residual in µs over every peer's earliest GossipSub receipt, the
    /// start time fitted, and how many receipts the source cannot reach over
    /// the mesh snapshot. A check of the model, not an estimator.
    pub(crate) fn fit_from(&self, source: PeerId) -> Option<(f64, usize)> {
        let s = self.peers.iter().position(|p| *p == source)?;
        let mut residuals = Vec::new();
        let mut unreachable = 0;
        for (j, t) in self.first_gossip.iter().enumerate() {
            let Some(t) = t else { continue };
            match self.predicted_delay(s, j) {
                Some(p) => residuals.push(t - p),
                None => unreachable += 1,
            }
        }
        if residuals.is_empty() {
            return None;
        }
        let t0 = residuals.iter().sum::<f64>() / residuals.len() as f64;
        let ssr: f64 = residuals.iter().map(|r| (r - t0).powi(2)).sum();
        Some(((ssr / residuals.len() as f64).sqrt(), unreachable))
    }

    /// One estimator's guess for a coalition, given as positions in `peers`.
    /// `seen` are the coalition's observations; only the stem rule uses them.
    fn guess<R: Rng>(
        &self,
        estimator: TimingEstimator,
        coalition: &[usize],
        seen: &[&Observation],
        rng: &mut R,
    ) -> Option<PeerId> {
        // As in `adversary`, a colluder knows it is not the source.
        let mut colluding = vec![false; self.peers.len()];
        for &j in coalition {
            colluding[j] = true;
        }
        let candidates: Vec<usize> = (0..self.peers.len()).filter(|s| !colluding[*s]).collect();
        if estimator == TimingEstimator::DelayStem && seen.iter().any(|o| o.stem) {
            let candidates: Vec<PeerId> = candidates.iter().map(|s| self.peers[*s]).collect();
            return adversary::guess(Estimator::FirstSpy, seen, &candidates, rng);
        }
        let receipts: Vec<(usize, f64)> = coalition
            .iter()
            .filter_map(|&j| self.first_gossip[j].map(|t| (j, t)))
            .collect();
        let best = match estimator {
            TimingEstimator::Delay | TimingEstimator::DelayStem => {
                best_fit(&receipts, &candidates, |s, j| self.predicted_delay(s, j), rng)
            }
            TimingEstimator::Hops => {
                best_fit(&receipts, &candidates, |s, j| self.predicted_hops(s, j), rng)
            }
        };
        best.map(|s| self.peers[s])
    }
}

/// The candidate whose predicted arrival times fit the receipts best. The
/// publish time is unknown, so it is fitted as the mean residual, and the score
/// is the sum of squared residuals around it. Candidates that cannot reach a
/// receipt's colluder are left out. Ties break uniformly: with one receipt,
/// every candidate fits perfectly.
fn best_fit<R: Rng>(
    receipts: &[(usize, f64)],
    candidates: &[usize],
    predict: impl Fn(usize, usize) -> Option<f64>,
    rng: &mut R,
) -> Option<usize> {
    if receipts.is_empty() {
        return None;
    }
    let mut best = f64::INFINITY;
    let mut top: Vec<usize> = Vec::new();
    let mut residuals = Vec::with_capacity(receipts.len());
    'candidates: for &s in candidates {
        residuals.clear();
        for &(j, t) in receipts {
            let Some(p) = predict(s, j) else { continue 'candidates };
            residuals.push(t - p);
        }
        let t0 = residuals.iter().sum::<f64>() / residuals.len() as f64;
        let ssr: f64 = residuals.iter().map(|r| (r - t0).powi(2)).sum();
        if ssr < best - TIE_US2 {
            best = ssr;
            top.clear();
            top.push(s);
        } else if ssr <= best + TIE_US2 {
            top.push(s);
        }
    }
    top.choose(rng).copied()
}

/// Sample colluding subsets of size `coalition_size`, drawn as in
/// `adversary::run_attack`, and score every timing estimator against the true
/// source. Returns the success rate per estimator.
pub(crate) fn run_attack<R: Rng>(
    model: &TimingModel,
    observations: &[Observation],
    source: PeerId,
    coalition_size: usize,
    trials: usize,
    rng: &mut R,
) -> HashMap<TimingEstimator, f64> {
    let pool: Vec<usize> = (0..model.peers.len()).filter(|i| model.peers[*i] != source).collect();
    let coalition_size = coalition_size.clamp(1, pool.len().max(1));

    let mut hits: HashMap<TimingEstimator, usize> =
        TimingEstimator::ALL.iter().map(|e| (*e, 0)).collect();
    let mut shuffled = pool;

    for _ in 0..trials {
        shuffled.shuffle(rng);
        // Positions in the shuffled order, not a set, so that the receipts
        // are summed in an order the seed fixes.
        let coalition: Vec<usize> = shuffled.iter().copied().take(coalition_size).collect();
        let members: HashSet<PeerId> = coalition.iter().map(|i| model.peers[*i]).collect();
        let seen: Vec<&Observation> = observations
            .iter()
            .filter(|o| members.contains(&o.observer))
            .collect();
        for estimator in TimingEstimator::ALL {
            if model.guess(estimator, &coalition, &seen, rng) == Some(source) {
                *hits.get_mut(&estimator).unwrap() += 1;
            }
        }
    }

    hits.into_iter()
        .map(|(e, h)| (e, h as f64 / trials as f64))
        .collect()
}

/// Each observer's earliest GossipSub receipt: µs after the publish, and the relay.
fn first_gossip_receipts(observations: &[Observation]) -> HashMap<PeerId, (f64, PeerId)> {
    let mut first: HashMap<PeerId, (f64, PeerId)> = HashMap::new();
    for o in observations.iter().filter(|o| !o.stem) {
        let t = o.at.as_nanos() as f64 / 1e3;
        let entry = first.entry(o.observer).or_insert((t, o.prop_id));
        if t < entry.0 {
            *entry = (t, o.prop_id);
        }
    }
    first
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    })
}

/// Mean link delay in µs, at least 1 so that hop counts still order candidates.
fn mean_link_us(topology: &Topology) -> f64 {
    let delays: Vec<f64> = topology
        .links
        .values()
        .flatten()
        .map(|(_, d)| d.as_secs_f64() * 1e6)
        .collect();
    if delays.is_empty() {
        return 1.0;
    }
    (delays.iter().sum::<f64>() / delays.len() as f64).max(1.0)
}

/// Fastest-path times from `s` to every peer; `u64::MAX` where unreachable.
fn dijkstra(adjacency: &[Vec<(usize, u64)>], s: usize) -> Vec<u64> {
    let mut dist = vec![u64::MAX; adjacency.len()];
    let mut heap = BinaryHeap::new();
    dist[s] = 0;
    heap.push(Reverse((0u64, s)));
    while let Some(Reverse((d, u))) = heap.pop() {
        if d > dist[u] {
            continue;
        }
        for &(v, w) in &adjacency[u] {
            let next = d + w;
            if next < dist[v] {
                dist[v] = next;
                heap.push(Reverse((next, v)));
            }
        }
    }
    dist
}

/// Fewest hops from `s` to every peer; `u32::MAX` where unreachable.
fn bfs(adjacency: &[Vec<(usize, u64)>], s: usize) -> Vec<u32> {
    let mut hops = vec![u32::MAX; adjacency.len()];
    let mut queue = VecDeque::from([s]);
    hops[s] = 0;
    while let Some(u) = queue.pop_front() {
        for &(v, _) in &adjacency[u] {
            if hops[v] == u32::MAX {
                hops[v] = hops[u] + 1;
                queue.push_back(v);
            }
        }
    }
    hops
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn pid(seed: u8) -> PeerId {
        PeerId::from_multihash(libp2p::multihash::Multihash::wrap(0x0, &[seed; 32]).unwrap())
            .unwrap()
    }

    /// Peers 0..n with symmetric links (a, b, delay in ms).
    fn graph(n: u8, edges: &[(u8, u8, u64)]) -> (Vec<PeerId>, Topology) {
        let peers: Vec<PeerId> = (0..n).map(pid).collect();
        let mut links: HashMap<PeerId, Vec<(PeerId, Duration)>> = HashMap::new();
        for &(a, b, ms) in edges {
            let (a, b, d) = (peers[a as usize], peers[b as usize], Duration::from_millis(ms));
            links.entry(a).or_default().push((b, d));
            links.entry(b).or_default().push((a, d));
        }
        (peers, Topology { links })
    }

    /// The trace a flood from `source` leaves: every other peer gets its first
    /// copy over the fastest path, `processing_us` per hop on top of the link
    /// delays, from the neighbour on that path, `t0_us` after the publish.
    fn flood(peers: &[PeerId], topology: &Topology, source: usize, processing_us: u64, t0_us: u64) -> Vec<Observation> {
        let model = TimingModel::new(topology, peers, &[], processing_us as f64, 1.0);
        let n = peers.len();
        let dist = |j: usize| model.delay_us[source * n + j];
        (0..n)
            .filter(|j| *j != source && dist(*j) != u64::MAX)
            .map(|j| {
                let relay = (0..n)
                    .find(|i| {
                        let w = topology.delay(&peers[*i], &peers[j]);
                        dist(*i) != u64::MAX
                            && w.is_some_and(|w| dist(*i) + w.as_micros() as u64 + processing_us == dist(j))
                    })
                    .unwrap();
                Observation {
                    observer: peers[j],
                    prop_id: peers[relay],
                    at: Duration::from_micros(t0_us + dist(j)),
                    stem: false,
                }
            })
            .collect()
    }

    fn seen_by<'a>(observations: &'a [Observation], peers: &[PeerId], coalition: &[usize]) -> Vec<&'a Observation> {
        let members: HashSet<PeerId> = coalition.iter().map(|i| peers[*i]).collect();
        observations.iter().filter(|o| members.contains(&o.observer)).collect()
    }

    /// The relay steps of a flood give back the processing constant it was
    /// made with, and the hop time of a line with equal links.
    #[test]
    fn calibration_recovers_the_per_hop_constants() {
        let (peers, topology) = graph(6, &[(0, 1, 20), (1, 2, 20), (2, 3, 20), (3, 4, 20), (4, 5, 20)]);
        let observations = flood(&peers, &topology, 2, 1500, 400);
        let model = TimingModel::calibrated(&topology, &peers, &observations);
        // Steps out of the source do not count: it has no receipt of its own.
        assert_eq!(model.relay_steps, 3);
        assert_eq!(model.processing_us, 1500.0);
        assert_eq!(model.hop_us, 21_500.0);
    }

    /// Colluders at both ends of a line pin the source down: the difference
    /// of their receipt times changes with every step along the line.
    #[test]
    fn delays_name_the_source_on_a_line() {
        let (peers, topology) =
            graph(7, &[(0, 1, 10), (1, 2, 30), (2, 3, 20), (3, 4, 50), (4, 5, 40), (5, 6, 60)]);
        let mut rng = StdRng::seed_from_u64(1);
        for source in 1..=5 {
            let observations = flood(&peers, &topology, source, 2000, 300_000);
            let model = TimingModel::calibrated(&topology, &peers, &observations);
            let seen = seen_by(&observations, &peers, &[0, 6]);
            let guess = model.guess(TimingEstimator::Delay, &[0, 6], &seen, &mut rng);
            assert_eq!(guess, Some(peers[source]), "source {source}");
        }
    }

    /// With equal links, hop counts carry the same information as delays.
    #[test]
    fn hops_name_the_source_on_a_line_with_equal_links() {
        let (peers, topology) = graph(7, &[(0, 1, 20), (1, 2, 20), (2, 3, 20), (3, 4, 20), (4, 5, 20), (5, 6, 20)]);
        let mut rng = StdRng::seed_from_u64(2);
        for source in 1..=5 {
            let observations = flood(&peers, &topology, source, 1000, 50_000);
            let model = TimingModel::calibrated(&topology, &peers, &observations);
            let seen = seen_by(&observations, &peers, &[0, 6]);
            let guess = model.guess(TimingEstimator::Hops, &[0, 6], &seen, &mut rng);
            assert_eq!(guess, Some(peers[source]), "source {source}");
        }
    }

    /// A star whose arms are two hops long, with colluders at the four tips.
    /// The hub and each middle peer predict a different pattern of receipt
    /// times: the tip of the source's own arm gets the message early.
    #[test]
    fn delays_and_hops_name_the_source_on_a_star_of_lines() {
        // Hub 0, middles 1..=4, tips 5..=8.
        let (peers, topology) = graph(
            9,
            &[(0, 1, 15), (0, 2, 35), (0, 3, 55), (0, 4, 25), (1, 5, 45), (2, 6, 10), (3, 7, 30), (4, 8, 70)],
        );
        let (uniform_peers, uniform) = graph(
            9,
            &[(0, 1, 20), (0, 2, 20), (0, 3, 20), (0, 4, 20), (1, 5, 20), (2, 6, 20), (3, 7, 20), (4, 8, 20)],
        );
        let tips = [5, 6, 7, 8];
        let mut rng = StdRng::seed_from_u64(3);
        for source in 0..=4 {
            let observations = flood(&peers, &topology, source, 1000, 10_000);
            let model = TimingModel::calibrated(&topology, &peers, &observations);
            let seen = seen_by(&observations, &peers, &tips);
            assert_eq!(model.guess(TimingEstimator::Delay, &tips, &seen, &mut rng), Some(peers[source]));

            let observations = flood(&uniform_peers, &uniform, source, 1000, 10_000);
            let model = TimingModel::calibrated(&uniform, &uniform_peers, &observations);
            let seen = seen_by(&observations, &uniform_peers, &tips);
            assert_eq!(
                model.guess(TimingEstimator::Hops, &tips, &seen, &mut rng),
                Some(uniform_peers[source])
            );
        }
    }

    /// On a plain star, every path from a leaf runs through the hub, so a leaf
    /// that does not collude predicts the same receipt differences as the hub:
    /// timing cannot tell them apart, and ties break uniformly. Only when all
    /// leaves collude is the hub named for sure.
    #[test]
    fn a_star_hub_ties_with_the_leaves_that_do_not_collude() {
        let (peers, topology) =
            graph(7, &[(0, 1, 10), (0, 2, 20), (0, 3, 30), (0, 4, 40), (0, 5, 50), (0, 6, 60)]);
        let observations = flood(&peers, &topology, 0, 1000, 0);
        let model = TimingModel::new(&topology, &peers, &observations, 1000.0, 20_000.0);
        let mut rng = StdRng::seed_from_u64(4);

        let leaves = [1, 2, 3, 4, 5, 6];
        let seen = seen_by(&observations, &peers, &leaves);
        for _ in 0..100 {
            assert_eq!(model.guess(TimingEstimator::Delay, &leaves, &seen, &mut rng), Some(peers[0]));
        }

        let some = [1, 2, 3];
        let seen = seen_by(&observations, &peers, &some);
        let trials = 20_000;
        let hub = (0..trials)
            .filter(|_| model.guess(TimingEstimator::Delay, &some, &seen, &mut rng) == Some(peers[0]))
            .count();
        let rate = hub as f64 / trials as f64;
        assert!((rate - 0.25).abs() < 0.02, "hub named in {rate}, not about 1/4");
    }

    /// A candidate that cannot reach a colluder over the mesh is never named,
    /// and a coalition without a single GossipSub receipt has nothing to fit.
    #[test]
    fn unreachable_candidates_are_left_out() {
        // Peer 3 has no mesh links.
        let (peers, topology) = graph(4, &[(0, 1, 10), (1, 2, 20)]);
        let observations = flood(&peers, &topology, 0, 1000, 0);
        let model = TimingModel::calibrated(&topology, &peers, &observations);
        let mut rng = StdRng::seed_from_u64(5);
        let seen = seen_by(&observations, &peers, &[2]);
        let guesses: HashSet<Option<PeerId>> = (0..200)
            .map(|_| model.guess(TimingEstimator::Delay, &[2], &seen, &mut rng))
            .collect();
        assert_eq!(guesses, HashSet::from([Some(peers[0]), Some(peers[1])]));
        assert_eq!(model.guess(TimingEstimator::Delay, &[3], &[], &mut rng), None);
    }

    /// With a colluder on the stem, the combined estimator names its stem
    /// predecessor, as stem-aware first-spy does; otherwise it is `timing_delay`.
    #[test]
    fn the_stem_rule_comes_first_in_the_combined_estimator() {
        let (peers, topology) =
            graph(7, &[(0, 1, 10), (1, 2, 30), (2, 3, 20), (3, 4, 50), (4, 5, 40), (5, 6, 60)]);
        let mut observations = flood(&peers, &topology, 2, 1000, 0);
        let model = TimingModel::calibrated(&topology, &peers, &observations);
        let mut rng = StdRng::seed_from_u64(6);
        let coalition = [0, 6];

        let seen = seen_by(&observations, &peers, &coalition);
        assert_eq!(model.guess(TimingEstimator::DelayStem, &coalition, &seen, &mut rng), Some(peers[2]));

        observations.push(Observation {
            observer: peers[6],
            prop_id: peers[4],
            at: Duration::from_millis(1),
            stem: true,
        });
        let model = TimingModel::calibrated(&topology, &peers, &observations);
        let seen = seen_by(&observations, &peers, &coalition);
        assert_eq!(model.guess(TimingEstimator::DelayStem, &coalition, &seen, &mut rng), Some(peers[4]));
        // The stem receipt, earlier than any gossip, does not enter the fit.
        assert_eq!(model.guess(TimingEstimator::Delay, &coalition, &seen, &mut rng), Some(peers[2]));
    }

    /// End to end through the sampler: a coalition of everyone but the source
    /// leaves one candidate, and the source is the only peer not drawn.
    #[test]
    fn run_attack_scores_every_estimator() {
        let (peers, topology) =
            graph(5, &[(0, 1, 10), (1, 2, 30), (2, 3, 20), (3, 4, 50)]);
        let observations = flood(&peers, &topology, 1, 1000, 0);
        let model = TimingModel::calibrated(&topology, &peers, &observations);
        let mut rng = StdRng::seed_from_u64(7);
        let res = run_attack(&model, &observations, peers[1], 4, 100, &mut rng);
        for estimator in TimingEstimator::ALL {
            assert_eq!(res[&estimator], 1.0, "{estimator}");
        }
        let fit = model.fit_from(peers[1]).unwrap();
        assert!(fit.0 < 1.0 && fit.1 == 0, "{fit:?}");
    }

    /// Link delays come from the listening ports, as in the transport.
    #[test]
    fn topology_takes_delays_from_the_ports() {
        let latency = LatencyRange { min_ms: 10, max_ms: 100 };
        let (a, b) = (pid(1), pid(2));
        let addrs = vec![
            (a, "/ip4/127.0.0.1/tcp/40001".parse().unwrap()),
            (b, "/ip4/127.0.0.1/tcp/40002".parse().unwrap()),
        ];
        let meshes = HashMap::from([(a, vec![b]), (b, vec![a, pid(3)])]);
        let topology = Topology::from_meshes(&addrs, &meshes, latency);
        assert_eq!(topology.delay(&a, &b), Some(latency.for_ports(40001, 40002)));
        assert_eq!(topology.delay(&b, &a), topology.delay(&a, &b));
        // Peer 3 never registered an address, so the link to it is unknown.
        assert_eq!(topology.links[&b].len(), 1);
    }
}
