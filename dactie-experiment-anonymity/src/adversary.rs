//! Post-hoc source-anonymity attack over recorded gossipsub observations.
//!
//! Threat model: a fraction `f` of peers is honest-but-curious and pools what it
//! sees. Each colluding peer knows only who relayed the measurement message to it
//! and when. It does not know the topology. The goal is to name the originator.
//!
//! The attack is pure post-processing of one recorded run, so thousands of
//! colluding subsets can be sampled per experiment without re-running the network.

use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Estimator {
    /// Guess uniformly at random from all candidate peers. Baseline: 1/|N|.
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

/// Run one estimator against a single colluding coalition's observations.
/// Returns `None` when the coalition saw nothing, which counts as a failure.
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
        Estimator::FirstSpy => seen
            .iter()
            .min_by_key(|o| o.at)
            .map(|o| o.prop_id),
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

/// Sample colluding subsets of size `ceil(f * |all_peers|)` and score every
/// estimator against the true source. Returns success rate per estimator.
pub(crate) fn run_attack<R: Rng>(
    observations: &[Observation],
    source: PeerId,
    all_peers: &[PeerId],
    f: f64,
    trials: usize,
    rng: &mut R,
) -> HashMap<Estimator, f64> {
    let n = all_peers.len();
    let coalition_size = ((f * n as f64).ceil() as usize).clamp(1, n);

    // The source cannot be a candidate for itself being unmasked by definition of
    // the guess, but it IS in the candidate pool an attacker would consider.
    let candidates: Vec<PeerId> = all_peers.to_vec();

    let mut hits: HashMap<Estimator, usize> = Estimator::ALL.iter().map(|e| (*e, 0)).collect();

    for _ in 0..trials {
        // A coalition is drawn from peers other than the source: the source
        // trivially knows it is the source, so including it inflates every rate.
        let mut pool: Vec<PeerId> = all_peers.iter().copied().filter(|p| *p != source).collect();
        pool.shuffle(rng);
        let coalition: Vec<PeerId> = pool.into_iter().take(coalition_size).collect();

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
        PeerId::from_multihash(
            libp2p::multihash::Multihash::wrap(0x0, &[seed; 32]).unwrap(),
        )
        .unwrap()
    }

    /// If every colluder was relayed to directly by the source and the earliest
    /// observation is one of them, first-spy must name the source every time.
    #[test]
    fn first_spy_finds_a_source_that_relayed_directly() {
        let source = pid(1);
        let peers: Vec<PeerId> = (1..=6).map(pid).collect();
        let observations: Vec<Observation> = (2..=6)
            .map(|i| Observation {
                observer: pid(i),
                prop_id: source,
                at: Duration::from_millis(i as u64 * 10),
            })
            .collect();

        let mut rng = StdRng::seed_from_u64(7);
        let res = run_attack(&observations, source, &peers, 1.0, 200, &mut rng);
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
            })
            .collect();

        let mut rng = StdRng::seed_from_u64(7);
        let res = run_attack(&observations, source, &peers, 1.0, 200, &mut rng);
        assert_eq!(res[&Estimator::FirstSpy], 0.0);
        assert_eq!(res[&Estimator::ObservedSet], 0.0);
    }

    /// The random baseline should sit near 1/N regardless of what was observed.
    #[test]
    fn random_baseline_tracks_one_over_n() {
        let source = pid(1);
        let peers: Vec<PeerId> = (1..=10).map(pid).collect();
        let observations = vec![Observation {
            observer: pid(2),
            prop_id: pid(3),
            at: Duration::from_millis(5),
        }];

        let mut rng = StdRng::seed_from_u64(11);
        let res = run_attack(&observations, source, &peers, 0.5, 20_000, &mut rng);
        let expected = 1.0 / peers.len() as f64;
        assert!(
            (res[&Estimator::Random] - expected).abs() < 0.02,
            "random baseline {} not near {}",
            res[&Estimator::Random],
            expected
        );
    }
}
