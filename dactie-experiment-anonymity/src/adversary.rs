//! Post-hoc source-anonymity attack over recorded gossipsub observations.
//!
//! Threat model: a coalition of `c` peers is honest-but-curious and pools what it
//! sees. Each colluding peer knows only who relayed the measurement message to it
//! and when. It does not know the topology. The goal is to name the originator.
//!
//! The attack is pure post-processing of one recorded run, so thousands of
//! colluding subsets can be sampled per experiment without re-running the network.
//!
//! The bound in the paper is stated over the coalition size, so `c` is the knob
//! the attack is parameterised by. `coalition_size_for` converts a fraction to
//! one; at `c = 1` the bound reduces to the published single-attacker form.

use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

/// One observation as an adversary would record it.
#[derive(Debug, Clone)]
pub(crate) struct Observation {
    /// The colluding peer that made the observation.
    pub(crate) observer: PeerId,
    /// The neighbour that relayed the message to `observer`.
    pub(crate) prop_id: PeerId,
    /// Arrival, relative to the publish instant.
    pub(crate) at: Duration,
    /// Received on the stem (Dandelion++) rather than through GossipSub.
    pub(crate) stem: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Estimator {
    /// Guess uniformly at random from the peers the coalition cannot rule out.
    /// Each colluding peer knows it did not publish, so the coalition excludes
    /// its own c members and the baseline is 1/(|N| - c), not 1/|N|.
    Random,
    /// Guess uniformly among the distinct relays the coalition saw. Tests
    /// whether merely being adjacent to the source leaks it.
    ObservedSet,
    /// The colluding peer that saw the message earliest guesses its own relay.
    /// The standard attack on flooding and gossip anonymity.
    FirstSpy,
    /// Guess the relay appearing most often across colluding observations. The
    /// source forwards to D peers, so it recurs when several colluders are its
    /// mesh neighbours.
    Plurality,
}

impl Estimator {
    pub(crate) const ALL: [Estimator; 4] = [
        Estimator::Random,
        Estimator::ObservedSet,
        Estimator::FirstSpy,
        Estimator::Plurality,
    ];
}

impl fmt::Display for Estimator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Estimator::Random => "random",
            Estimator::ObservedSet => "observed_set",
            Estimator::FirstSpy => "first_spy",
            Estimator::Plurality => "plurality",
        };
        write!(f, "{s}")
    }
}

/// The coalition size a fraction `f` of an `n`-peer network corresponds to.
///
/// The pool a coalition is drawn from excludes the source, so it holds `n - 1`
/// peers and that, not `n`, is the ceiling. Bounded below by 1: a coalition of
/// nobody has nothing to pool.
pub(crate) fn coalition_size_for(f: f64, n: usize) -> usize {
    clamp_coalition_size((f * n as f64).ceil() as usize, n)
}

/// Hold an absolute coalition size inside what the pool can supply.
///
/// Use this rather than routing a size through `coalition_size_for`: c/n*n does
/// not round-trip through `ceil` for most (c, n), so c=7 of 25 would come back
/// as 8.
pub(crate) fn clamp_coalition_size(c: usize, n: usize) -> usize {
    c.clamp(1, n.saturating_sub(1).max(1))
}

/// Run one estimator against a single colluding coalition's observations.
///
/// Returns `None` when the estimator has nothing to go on, which counts as a
/// failure. `Random` is the exception: a random guesser needs no observations,
/// so it always answers and lands on exactly 1/|N|. That is what makes it the
/// baseline, so do not "fix" it to depend on `seen`.
fn guess<R: Rng>(
    estimator: Estimator,
    seen: &[&Observation],
    candidates: &[PeerId],
    rng: &mut R,
) -> Option<PeerId> {
    match estimator {
        Estimator::Random => candidates.choose(rng).copied(),
        Estimator::ObservedSet => {
            let mut distinct: Vec<PeerId> = seen.iter().map(|o| o.prop_id).collect();
            distinct.sort_unstable();
            distinct.dedup();
            distinct.choose(rng).copied()
        }
        Estimator::FirstSpy => {
            // A colluder on the stem names its stem predecessor; gossip
            // observations only count when no colluder sat on the stem.
            let on_stem: Vec<&Observation> = seen.iter().copied().filter(|o| o.stem).collect();
            let seen: Vec<&Observation> = if on_stem.is_empty() { seen.to_vec() } else { on_stem };
            let earliest = seen.iter().map(|o| o.at).min()?;
            // Ties break uniformly, as they do for plurality. Observations
            // arrive over an unordered channel, so picking the first minimum
            // would resolve a tie by channel order rather than by chance.
            let first: Vec<PeerId> = seen
                .iter()
                .filter(|o| o.at == earliest)
                .map(|o| o.prop_id)
                .collect();
            first.choose(rng).copied()
        }
        Estimator::Plurality => {
            let mut counts: HashMap<PeerId, usize> = HashMap::new();
            for o in seen {
                *counts.entry(o.prop_id).or_insert(0) += 1;
            }
            let max = counts.values().copied().max()?;
            // Ties are broken uniformly, so a tie does not silently favour
            // whichever peer id happens to sort first.
            let top: Vec<PeerId> = counts
                .into_iter()
                .filter(|(_, c)| *c == max)
                .map(|(p, _)| p)
                .collect();
            top.choose(rng).copied()
        }
    }
}

/// Sample colluding subsets of size `coalition_size` and score every estimator
/// against the true source. Returns success rate per estimator.
pub(crate) fn run_attack<R: Rng>(
    observations: &[Observation],
    source: PeerId,
    all_peers: &[PeerId],
    coalition_size: usize,
    trials: usize,
    rng: &mut R,
) -> HashMap<Estimator, f64> {
    // A coalition is drawn from peers other than the source: the source
    // trivially knows it is the source, so including it inflates every rate.
    let pool: Vec<PeerId> = all_peers.iter().copied().filter(|p| *p != source).collect();
    let coalition_size = coalition_size.clamp(1, pool.len().max(1));

    let mut hits: HashMap<Estimator, usize> = Estimator::ALL.iter().map(|e| (*e, 0)).collect();
    let mut shuffled = pool;

    for _ in 0..trials {
        shuffled.shuffle(rng);
        // Membership is tested once per observation per trial, so a set keeps
        // this O(1) rather than linear in the coalition. At N=850 and f=0.5 the
        // linear form costs on the order of 1e9 comparisons across a sweep.
        let coalition: HashSet<PeerId> = shuffled.iter().copied().take(coalition_size).collect();

        // A colluding peer knows it is not the source, so the coalition rules
        // out its own members. The source itself is never in the coalition and
        // therefore always remains a candidate.
        let candidates: Vec<PeerId> = all_peers
            .iter()
            .copied()
            .filter(|p| !coalition.contains(p))
            .collect();

        let seen: Vec<&Observation> = observations
            .iter()
            .filter(|o| coalition.contains(&o.observer))
            .collect();

        for estimator in Estimator::ALL {
            if let Some(g) = guess(estimator, &seen, &candidates, rng) {
                if g == source {
                    *hits.get_mut(&estimator).unwrap() += 1;
                }
            }
        }
    }

    hits.into_iter()
        .map(|(e, h)| (e, h as f64 / trials as f64))
        .collect()
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

    /// If every colluder was relayed to directly by the source and the earliest
    /// observation is one of them, first-spy must name the source every time.
    /// With the stem on, a colluder on the stem names its stem predecessor, and
    /// gossip observations are ignored, however early they are.
    #[test]
    fn first_spy_prefers_the_stem() {
        let mut rng = StdRng::seed_from_u64(9);
        let (source, relay, b, c) = (pid(1), pid(2), pid(3), pid(4));
        let on_stem = Observation { observer: b, prop_id: relay, at: Duration::from_millis(20), stem: true };
        let gossip = Observation { observer: c, prop_id: source, at: Duration::from_millis(5), stem: false };
        let candidates = [source, relay];
        assert_eq!(guess(Estimator::FirstSpy, &[&on_stem, &gossip], &candidates, &mut rng), Some(relay));
        let first_hop = Observation { observer: b, prop_id: source, at: Duration::from_millis(30), stem: true };
        assert_eq!(guess(Estimator::FirstSpy, &[&first_hop, &gossip], &candidates, &mut rng), Some(source));
        assert_eq!(guess(Estimator::FirstSpy, &[&gossip], &candidates, &mut rng), Some(source));
    }

    #[test]
    fn first_spy_finds_a_source_that_relayed_directly() {
        let source = pid(1);
        let peers: Vec<PeerId> = (1..=6).map(pid).collect();
        let observations: Vec<Observation> = (2..=6)
            .map(|i| Observation {
                observer: pid(i),
                prop_id: source,
                at: Duration::from_millis(i as u64 * 10),
                stem: false,
            })
            .collect();

        let mut rng = StdRng::seed_from_u64(7);
        let res = run_attack(&observations, source, &peers, 5, 200, &mut rng);
        assert_eq!(res[&Estimator::FirstSpy], 1.0);
        assert_eq!(res[&Estimator::Plurality], 1.0);
    }

    /// When no colluder was adjacent to the source, first-spy cannot name it:
    /// every relay they saw was some other peer.
    #[test]
    fn first_spy_fails_when_source_is_not_adjacent() {
        let source = pid(1);
        let relay = pid(2);
        let peers: Vec<PeerId> = (1..=6).map(pid).collect();
        let observations: Vec<Observation> = (3..=6)
            .map(|i| Observation {
                observer: pid(i),
                prop_id: relay,
                at: Duration::from_millis(i as u64 * 10),
                stem: false,
            })
            .collect();

        let mut rng = StdRng::seed_from_u64(7);
        let res = run_attack(&observations, source, &peers, 5, 200, &mut rng);
        assert_eq!(res[&Estimator::FirstSpy], 0.0);
        assert_eq!(res[&Estimator::ObservedSet], 0.0);
    }

    /// The random baseline sits near 1/(N - c): a coalition rules out its own
    /// members, and what it observed is irrelevant to a uniform guess.
    #[test]
    fn random_baseline_tracks_one_over_n_minus_c() {
        let source = pid(1);
        let peers: Vec<PeerId> = (1..=10).map(pid).collect();
        let observations = vec![Observation {
            observer: pid(2),
            prop_id: pid(3),
            at: Duration::from_millis(5),
            stem: false,
        }];

        let mut rng = StdRng::seed_from_u64(11);
        let coalition_size = 5;
        let res = run_attack(
            &observations,
            source,
            &peers,
            coalition_size,
            20_000,
            &mut rng,
        );
        let expected = 1.0 / (peers.len() - coalition_size) as f64;
        assert!(
            (res[&Estimator::Random] - expected).abs() < 0.02,
            "random baseline {} not near {}",
            res[&Estimator::Random],
            expected
        );
    }

    /// Two observations share the earliest arrival, one of them from the source.
    /// Uniform tie-breaking must split them rather than letting the order the
    /// observations happen to sit in decide.
    #[test]
    fn first_spy_breaks_ties_uniformly() {
        let source = pid(1);
        let other = pid(2);
        let peers: Vec<PeerId> = (1..=6).map(pid).collect();
        let observations = vec![
            Observation {
                observer: pid(3),
                prop_id: other,
                at: Duration::from_millis(10),
                stem: false,
            },
            Observation {
                observer: pid(4),
                prop_id: source,
                at: Duration::from_millis(10),
                stem: false,
            },
        ];

        let mut rng = StdRng::seed_from_u64(3);
        let res = run_attack(&observations, source, &peers, 5, 20_000, &mut rng);
        assert!(
            (res[&Estimator::FirstSpy] - 0.5).abs() < 0.02,
            "first-spy tie split {} not near 0.5",
            res[&Estimator::FirstSpy]
        );
    }

    /// The coalition is drawn from the n-1 peers that are not the source, so a
    /// fraction of 1.0 must ask for n-1 and not n.
    #[test]
    fn coalition_size_is_capped_by_the_pool_not_the_network() {
        assert_eq!(coalition_size_for(1.0, 12), 11);
        assert_eq!(coalition_size_for(0.05, 12), 1);
        assert_eq!(coalition_size_for(0.05, 850), 43);
        assert_eq!(coalition_size_for(0.001, 850), 1);
        // A fraction small enough to round to nothing still yields one peer.
        assert_eq!(coalition_size_for(0.0, 40), 1);
    }

    /// An absolute size must survive unchanged, which routing it through a
    /// fraction would not guarantee: ceil(7 / 25 * 25) is 8.
    #[test]
    fn absolute_coalition_sizes_are_not_perturbed() {
        for n in 2..=200usize {
            for c in 1..n {
                assert_eq!(clamp_coalition_size(c, n), c, "c={c} n={n}");
            }
        }
        assert_eq!(clamp_coalition_size(999, 12), 11);
        assert_eq!(clamp_coalition_size(0, 12), 1);
    }
}
