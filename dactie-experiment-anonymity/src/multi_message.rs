//! Attacks across repeated messages of one source, and PeerID rotation.
//!
//! A single-message run gives a coalition one look at the source. Members
//! publish repeatedly, though, and a coalition that can link messages to one
//! unknown sender (same writing style, same topic) can pool its evidence over
//! them: an intersection attack, here a plurality vote over the per-message
//! first-spy guesses, and a statistical-disclosure attack, which compares how
//! often each peer relays the linked messages to the coalition with how often
//! it relays everyone else's. Fixed stem routes within an epoch are meant to
//! defeat both; successors redrawn between messages are not (docs/DANDELION.md).
//!
//! A member may also spread its messages over up to k = 5 PeerIDs per
//! registration epoch. Success is therefore scored twice: at the PeerID level
//! (the guess is the PeerID that sent the latest message) and at the member
//! level (the guess is any of the member's PeerIDs).
//!
//! One run serves every k. K PeerIDs, K the largest k asked for, each publish
//! M numbered messages in rounds. For a given k the member owns the first k of
//! them, and its message j is round j / k of PeerID j % k; the other K - k are
//! just other peers. Every k is attacked on the same network and coalitions,
//! so the comparison between them is paired.

use crate::adversary::{clamp_coalition_size, coalition_size_for, guess, Estimator, Observation};
use crate::experiment::{start_network, Network};
use crate::peer::{ControlSignal, MeshConfig, Payload, PubInterval};
use crate::stem::{StemConfig, EPOCH_JITTER};
use futures::future::join_all;
use libp2p::PeerId;
use rand::rngs::StdRng;
use rand::seq::{IteratorRandom, SliceRandom};
use rand::{Rng, SeedableRng};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use tokio::time::{sleep, sleep_until, Duration, Instant};

#[derive(clap::Args, Debug, Clone)]
pub(crate) struct MultiArgs {
    /// Multi-message mode: the source member publishes this many measurement
    /// messages per run (M), and the attack pools them, see multi_message.rs.
    /// Writes multi_message_*.csv instead of the single-message outputs.
    #[clap(long)]
    pub(crate) messages_per_source: Option<usize>,

    /// PeerIDs k the member spreads its M messages over, round robin (DACTIE
    /// allows at most 5 per registration epoch). One run serves all values.
    #[clap(long, value_delimiter = ',', default_values_t = [1usize])]
    pub(crate) peer_ids_per_source: Vec<usize>,

    /// Seconds between two messages of the same PeerID. For successors redrawn
    /// between messages, use at least --stem-epoch-secs plus the 30 s of epoch
    /// jitter; for one stem epoch, keep --time plus M times this below it.
    #[clap(long, default_value_t = 5.0)]
    pub(crate) message_spacing_secs: f64,
}

/// Who publishes which numbered message when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Plan {
    /// Messages per member, M.
    pub(crate) messages: usize,
    /// PeerIDs that publish: the largest k evaluated.
    pub(crate) peer_ids: usize,
    /// Between two messages of the same PeerID.
    pub(crate) spacing: Duration,
}

impl Plan {
    /// The number of the message PeerID `i` publishes in round `round`.
    fn seq(&self, round: usize, i: usize) -> u32 {
        (round * self.peer_ids + i) as u32
    }

    /// When PeerID `i` publishes in round `round`, after the first message.
    /// The PeerIDs of a round take turns, so no two messages leave at once.
    fn offset(&self, round: usize, i: usize) -> Duration {
        self.spacing * round as u32 + self.spacing.mul_f64(i as f64 / self.peer_ids as f64)
    }

    /// The member's message j when it owns the first k PeerIDs: the index of
    /// the PeerID that sends it, and its number.
    pub(crate) fn member_message(&self, j: usize, k: usize) -> (usize, u32) {
        (j % k, self.seq(j / k, j % k))
    }

    /// Every message of the run in publication order: (offset, PeerID, number).
    fn schedule(&self) -> Vec<(Duration, usize, u32)> {
        (0..self.messages)
            .flat_map(|r| (0..self.peer_ids).map(move |i| (r, i)))
            .map(|(r, i)| (self.offset(r, i), i, self.seq(r, i)))
            .collect()
    }
}

/// The stem regime a configuration falls in, for the CSV.
///
/// Epochs start with the first message a peer routes, which is after the peers
/// were told to run, at most `warmup` before the first measurement message. If
/// the last one leaves within the shortest epoch after that, every peer keeps
/// its routes for all M messages. If two messages of a PeerID are further apart
/// than the longest epoch, every peer has drawn new successors and a new role
/// in between. Anything else is a mix of both.
pub(crate) fn regime(stem: &StemConfig, plan: &Plan, warmup: Duration) -> &'static str {
    if !stem.is_on() {
        "no_stem"
    } else if warmup + plan.spacing * plan.messages as u32 <= stem.epoch {
        "stem_fixed"
    } else if plan.spacing >= stem.epoch + EPOCH_JITTER {
        "stem_rotating"
    } else {
        "stem_mixed"
    }
}

/// What each peer received of the background traffic, by relay.
#[derive(Debug, Default)]
pub(crate) struct Background {
    /// Per observer, its receipts through GossipSub and on the stem.
    observers: HashMap<PeerId, (Tally, Tally)>,
}

/// One observer's background receipts on the stem or through GossipSub.
#[derive(Debug, Default)]
struct Tally {
    total: u64,
    from: HashMap<PeerId, u64>,
}

impl Background {
    pub(crate) fn record(&mut self, observer: PeerId, relay: PeerId, stem: bool) {
        let tally = self.observers.entry(observer).or_default();
        let tally = if stem { &mut tally.1 } else { &mut tally.0 };
        tally.total += 1;
        *tally.from.entry(relay).or_insert(0) += 1;
    }

    /// The share of the colluders' background receipts, on the stem or
    /// through GossipSub, that came from `relay`.
    pub(crate) fn share(&self, colluders: &[PeerId], relay: &PeerId, stem: bool) -> f64 {
        let (mut from, mut total) = (0, 0);
        for (gossip, on_stem) in colluders.iter().filter_map(|c| self.observers.get(c)) {
            let tally = if stem { on_stem } else { gossip };
            from += tally.from.get(relay).copied().unwrap_or(0);
            total += tally.total;
        }
        if total == 0 {
            0.0
        } else {
            from as f64 / total as f64
        }
    }
}

/// How the coalition names the sender after m linked messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Aggregate {
    /// Uniform guess among the peers outside the coalition.
    Random,
    /// First-spy on message m alone: the single-message attack, no pooling.
    FirstSpy,
    /// The first-spy guess named most often over messages 1..m.
    Vote,
    /// Statistical disclosure over messages 1..m, see `Evidence`.
    Disclosure,
}

impl Aggregate {
    pub(crate) const ALL: [Aggregate; 4] = [
        Aggregate::Random,
        Aggregate::FirstSpy,
        Aggregate::Vote,
        Aggregate::Disclosure,
    ];
}

impl fmt::Display for Aggregate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Aggregate::Random => "random",
            Aggregate::FirstSpy => "first_spy",
            Aggregate::Vote => "first_spy_vote",
            Aggregate::Disclosure => "disclosure",
        };
        write!(f, "{s}")
    }
}

/// The guess named most often. Ties break uniformly, as for the
/// single-message estimators.
pub(crate) fn plurality<R: Rng>(votes: &HashMap<PeerId, usize>, rng: &mut R) -> Option<PeerId> {
    let max = votes.values().copied().max()?;
    let top: Vec<PeerId> = votes
        .iter()
        .filter(|(_, v)| **v == max)
        .map(|(p, _)| *p)
        .collect();
    top.choose(rng).copied()
}

/// What the statistical-disclosure attack has gathered over the linked
/// messages so far: Danezis' attack (2003) with relays in place of receivers.
///
/// Each message counts its relays as first-spy judges it: by the stem
/// observations if a colluder sat on the stem, else by the gossip ones. Every
/// message weighs one in all, shared among the relays its observations name,
/// so a message that many colluders received through peers near the diffuser
/// does not outweigh one a colluder saw at the first stem hop.
#[derive(Debug, Default)]
pub(crate) struct Evidence {
    /// Per relay, its summed share of the messages.
    shares: HashMap<PeerId, f64>,
    /// Messages judged on stem observations and on gossip observations.
    on_stem: usize,
    in_gossip: usize,
}

impl Evidence {
    /// Add the coalition's observations of one more linked message.
    pub(crate) fn add(&mut self, seen: &[&Observation]) {
        let stem = seen.iter().any(|o| o.stem);
        let judged: Vec<&Observation> = seen.iter().copied().filter(|o| o.stem == stem).collect();
        if judged.is_empty() {
            return;
        }
        for o in &judged {
            *self.shares.entry(o.prop_id).or_insert(0.0) += 1.0 / judged.len() as f64;
        }
        if stem {
            self.on_stem += 1;
        } else {
            self.in_gossip += 1;
        }
    }

    /// The relay whose shares most exceed what its share of the coalition's
    /// background receipts of the same kind predicts. Without that baseline,
    /// a peer that relays much of everyone's traffic to the coalition, e.g.
    /// one in several colluders' meshes, would be named for every source.
    /// `background(p, stem)` is p's background share on the stem or in gossip.
    /// Colluders are never named, since each knows it is not the source.
    pub(crate) fn disclose<R: Rng>(
        &self,
        mut background: impl FnMut(&PeerId, bool) -> f64,
        coalition: &HashSet<PeerId>,
        rng: &mut R,
    ) -> Option<PeerId> {
        let scores: Vec<(PeerId, f64)> = self
            .shares
            .iter()
            .filter(|(p, _)| !coalition.contains(*p))
            .map(|(p, share)| {
                let expected = self.on_stem as f64 * background(p, true)
                    + self.in_gossip as f64 * background(p, false);
                (*p, share - expected)
            })
            .collect();
        let max = scores
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        // Scores are sums of fractions, so equal evidence can differ in the last bits.
        let top: Vec<PeerId> = scores
            .iter()
            .filter(|(_, s)| max - *s <= 1e-9)
            .map(|(p, _)| *p)
            .collect();
        top.choose(rng).copied()
    }
}

/// One of the member's messages as the coalition links it.
pub(crate) struct Linked<'a> {
    /// The PeerID that sent it.
    pub(crate) sender: PeerId,
    pub(crate) observations: &'a [Observation],
}

/// Success rates of one estimator after m messages.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Success {
    /// The guess is the PeerID that sent message m.
    pub(crate) peer_id: f64,
    /// The guess is one of the member's PeerIDs.
    pub(crate) member: f64,
}

/// The observations of one message by observer.
type ByObserver<'a> = HashMap<PeerId, Vec<&'a Observation>>;

/// Every estimator's guess after messages 1..=m, for each m, for one coalition.
fn guesses<R: Rng>(
    linked: &[ByObserver],
    colluders: &[PeerId],
    coalition: &HashSet<PeerId>,
    candidates: &[PeerId],
    background: &Background,
    rng: &mut R,
) -> Vec<[Option<PeerId>; 4]> {
    let mut votes: HashMap<PeerId, usize> = HashMap::new();
    let mut evidence = Evidence::default();
    let mut shares: HashMap<(PeerId, bool), f64> = HashMap::new();
    let mut out = Vec::with_capacity(linked.len());
    for message in linked {
        let seen: Vec<&Observation> = colluders
            .iter()
            .filter_map(|c| message.get(c))
            .flatten()
            .copied()
            .collect();
        let first = guess(Estimator::FirstSpy, &seen, candidates, rng);
        if let Some(g) = first {
            *votes.entry(g).or_insert(0) += 1;
        }
        evidence.add(&seen);
        let random = candidates.choose(rng).copied();
        let vote = plurality(&votes, rng);
        let sda = evidence.disclose(
            |p, stem| {
                *shares
                    .entry((*p, stem))
                    .or_insert_with(|| background.share(colluders, p, stem))
            },
            coalition,
            rng,
        );
        out.push([random, first, vote, sda]);
    }
    out
}

/// Sample coalitions of `coalition_size` and score every estimator after each
/// prefix of the linked messages. Entry m - 1 holds the rates after m messages.
pub(crate) fn run_multi_attack<R: Rng>(
    linked: &[Linked],
    member: &[PeerId],
    all_peers: &[PeerId],
    background: &Background,
    coalition_size: usize,
    trials: usize,
    rng: &mut R,
) -> Vec<HashMap<Aggregate, Success>> {
    // None of the member's PeerIDs colludes against it.
    let pool: Vec<PeerId> = all_peers
        .iter()
        .copied()
        .filter(|p| !member.contains(p))
        .collect();
    let coalition_size = coalition_size.clamp(1, pool.len().max(1));
    let indexed: Vec<ByObserver> = linked
        .iter()
        .map(|l| {
            let mut by: ByObserver = HashMap::new();
            for o in l.observations {
                by.entry(o.observer).or_default().push(o);
            }
            by
        })
        .collect();

    let mut hits: Vec<HashMap<Aggregate, (usize, usize)>> =
        vec![Aggregate::ALL.iter().map(|e| (*e, (0, 0))).collect(); linked.len()];
    let mut shuffled = pool;
    for _ in 0..trials {
        shuffled.shuffle(rng);
        let colluders = &shuffled[..coalition_size];
        let coalition: HashSet<PeerId> = colluders.iter().copied().collect();
        let candidates: Vec<PeerId> = all_peers
            .iter()
            .copied()
            .filter(|p| !coalition.contains(p))
            .collect();
        let rows = guesses(
            &indexed,
            colluders,
            &coalition,
            &candidates,
            background,
            rng,
        );
        for (j, row) in rows.into_iter().enumerate() {
            for (estimator, g) in Aggregate::ALL.into_iter().zip(row) {
                let h = hits[j].get_mut(&estimator).unwrap();
                if g == Some(linked[j].sender) {
                    h.0 += 1;
                }
                if g.is_some_and(|g| member.contains(&g)) {
                    h.1 += 1;
                }
            }
        }
    }

    hits.into_iter()
        .map(|per_m| {
            per_m
                .into_iter()
                .map(|(e, (p, m))| {
                    let success = Success {
                        peer_id: p as f64 / trials.max(1) as f64,
                        member: m as f64 / trials.max(1) as f64,
                    };
                    (e, success)
                })
                .collect()
        })
        .collect()
}

/// What one multi-message run yields.
pub(crate) struct MultiOutcome {
    /// Observations per numbered message, arrival relative to its publication.
    pub(crate) messages: HashMap<u32, Vec<Observation>>,
    pub(crate) background: Background,
    /// The PeerIDs that publish, indexed as in `Plan`.
    pub(crate) sources: Vec<PeerId>,
    pub(crate) all_peers: Vec<PeerId>,
}

impl MultiOutcome {
    /// The smallest share of the other peers that received a message of the
    /// run. Below 1.0 the run measures propagation failure, as in a
    /// single-message run.
    pub(crate) fn delivery_min(&self, plan: &Plan) -> f32 {
        let expected = self.all_peers.len().saturating_sub(1).max(1);
        plan.schedule()
            .iter()
            .map(|(_, _, seq)| {
                let receivers: HashSet<PeerId> = self
                    .messages
                    .get(seq)
                    .map(|obs| obs.iter().map(|o| o.observer).collect())
                    .unwrap_or_default();
                receivers.len() as f32 / expected as f32
            })
            .fold(1.0, f32::min)
    }
}

/// Start a network, publish the plan's messages, and collect what every peer
/// saw of them and of the background traffic.
pub(crate) async fn experiment_multi(
    peer_n: usize,
    time_before_stop: Duration,
    pub_interval: PubInterval,
    mesh: MeshConfig,
    plan: Plan,
) -> Result<MultiOutcome, Box<dyn Error>> {
    let Network {
        tasks,
        all_peers,
        last,
        control_tx,
        response_tx,
        mut response_rx,
    } = start_network(
        peer_n,
        time_before_stop,
        pub_interval,
        mesh.with_background_tally(true),
    )
    .await?;

    // The peer started last is the first PeerID, as it is the source of a
    // single-message run; the others are drawn at random.
    let mut sources = vec![last];
    sources.extend(
        all_peers
            .iter()
            .copied()
            .filter(|p| *p != last)
            .choose_multiple(&mut rand::thread_rng(), plan.peer_ids - 1),
    );

    // Background receipts arrive at the background rate times N, so they are
    // tallied as they come instead of piling up until the run ends.
    let collector = tokio::spawn(async move {
        let mut numbered = Vec::new();
        let mut background = Background::default();
        while let Some(out) = response_rx.recv().await {
            match out.payload {
                Payload::Numbered(n) => numbered.push((n, out)),
                Payload::Background => background.record(out.observer, out.prop_id, out.stem),
                Payload::Measurement => {}
            }
        }
        (numbered, background)
    });

    let start = Instant::now();
    let mut published: HashMap<u32, (Instant, PeerId)> = HashMap::new();
    for (offset, i, seq) in plan.schedule() {
        sleep_until(start + offset).await;
        published.insert(seq, (Instant::now(), sources[i]));
        control_tx
            .send(ControlSignal::PubSeq(sources[i], seq))
            .unwrap();
    }

    sleep(Duration::from_secs(30)).await;
    control_tx.send(ControlSignal::Stop).unwrap();
    join_all(tasks).await;
    println!("peers stopped");
    drop(response_tx);
    let (numbered, background) = collector.await?;

    let mut messages: HashMap<u32, Vec<Observation>> = HashMap::new();
    for (n, out) in numbered {
        let Some((at, sender)) = published.get(&n) else {
            continue;
        };
        // The diffuser's copy reaching the sender is no observation.
        if out.observer == *sender {
            continue;
        }
        messages.entry(n).or_default().push(Observation {
            observer: out.observer,
            prop_id: out.prop_id,
            at: out.at.saturating_duration_since(*at),
            stem: out.stem,
        });
    }

    Ok(MultiOutcome {
        messages,
        background,
        sources,
        all_peers,
    })
}

pub(crate) struct MultiSeriesArgs {
    pub(crate) min_n: usize,
    pub(crate) max_n: usize,
    pub(crate) n_step_size: usize,
    pub(crate) repetitions: usize,
    pub(crate) time: u64,
    pub(crate) interval: PubInterval,
    pub(crate) mesh: MeshConfig,
    pub(crate) out_path: PathBuf,
    pub(crate) adversary_fractions: Vec<f64>,
    pub(crate) coalition_sizes: Vec<usize>,
    pub(crate) adversary_trials: usize,
    pub(crate) adversary_seed: u64,
    pub(crate) min_delivery: f32,
    pub(crate) messages: usize,
    pub(crate) multi: MultiArgs,
}

/// The multi-message counterpart of the experiment series: one network run
/// per repetition and network size, attacked for every k and coalition size.
pub(crate) async fn start_multi_series(args: MultiSeriesArgs) -> Result<(), Box<dyn Error>> {
    let ks: BTreeSet<usize> = args
        .multi
        .peer_ids_per_source
        .iter()
        .map(|k| (*k).max(1))
        .collect();
    let plan = Plan {
        messages: args.messages.max(1),
        peer_ids: *ks.last().unwrap_or(&1),
        spacing: Duration::from_secs_f64(args.multi.message_spacing_secs.max(0.001)),
    };
    // A coalition needs at least one peer that is not the member's.
    if plan.peer_ids + 1 >= args.min_n {
        return Err(format!(
            "--peer-ids-per-source {} leaves no room for a coalition in {} peers",
            plan.peer_ids, args.min_n
        )
        .into());
    }
    let stem = args.mesh.stem;
    let regime = regime(&stem, &plan, Duration::from_secs(args.time));
    println!(
        "Multi-message runs: M={} messages per member, k in {ks:?}, spacing {:?}, regime {regime}",
        plan.messages, plan.spacing
    );
    if regime == "stem_mixed" {
        eprintln!(
            "WARNING: some stem routes change between messages and some do not. For one \
             epoch, keep --time plus M times --message-spacing-secs below --stem-epoch-secs; \
             for new successors between messages, set --message-spacing-secs to at least \
             --stem-epoch-secs plus {} s.",
            EPOCH_JITTER.as_secs()
        );
    }

    let mut path = args.out_path.clone();
    path.push(format!(
        "multi_message_{}_{}_{}_D{}_{:?}_q{}_e{}_s{}_M{}.csv",
        args.min_n,
        args.max_n,
        args.time,
        args.mesh.n,
        args.interval,
        stem.q,
        stem.epoch.as_secs(),
        args.multi.message_spacing_secs,
        plan.messages
    ));
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    writeln!(
        &file,
        "total_peers,mesh_n,mesh_n_low,mesh_n_high,stem_q,stem_epoch_secs,spacing_secs,regime,run,k,coalition_size,m,estimator,success_peer_id,success_member,delivery_min,trials"
    )?;

    for n_peers in (args.min_n..=args.max_n).step_by(args.n_step_size) {
        for i in 0..args.repetitions {
            let run = i + 1;
            println!(
                "Starting multi-message run {run} from {}, with peer_n={n_peers}, regime {regime}",
                args.repetitions
            );
            let outcome = experiment_multi(
                n_peers,
                Duration::from_secs(args.time),
                args.interval.clone(),
                args.mesh,
                plan,
            )
            .await?;
            let delivery = outcome.delivery_min(&plan);
            println!(
                "Delivery: at least {:.1}% of {} peers per message",
                delivery * 100.0,
                n_peers - 1
            );
            if delivery < args.min_delivery {
                eprintln!(
                    "WARNING: a measurement message reached only {:.1}% of peers (threshold \
                     {:.1}%). Filter this run out on delivery_min.",
                    delivery * 100.0,
                    args.min_delivery * 100.0
                );
            }

            let sizes: BTreeSet<usize> = args
                .adversary_fractions
                .iter()
                .map(|f| coalition_size_for(*f, n_peers))
                .chain(
                    args.coalition_sizes
                        .iter()
                        .map(|c| clamp_coalition_size(*c, n_peers)),
                )
                .collect();
            for k in &ks {
                let k = *k;
                let member = &outcome.sources[..k];
                let linked: Vec<Linked> = (0..plan.messages)
                    .map(|j| {
                        let (i, seq) = plan.member_message(j, k);
                        Linked {
                            sender: outcome.sources[i],
                            observations: outcome.messages.get(&seq).map_or(&[], |o| o.as_slice()),
                        }
                    })
                    .collect();
                for c in &sizes {
                    let c = *c;
                    let mut hasher = DefaultHasher::new();
                    (args.adversary_seed, n_peers, run, k, c).hash(&mut hasher);
                    let mut rng = StdRng::seed_from_u64(hasher.finish());
                    let rates = run_multi_attack(
                        &linked,
                        member,
                        &outcome.all_peers,
                        &outcome.background,
                        c,
                        args.adversary_trials,
                        &mut rng,
                    );
                    for (j, per_m) in rates.iter().enumerate() {
                        for estimator in Aggregate::ALL {
                            let s = per_m[&estimator];
                            writeln!(
                                &file,
                                "{n_peers},{n},{low},{high},{q},{e},{sp},{regime},{run},{k},{c},{m},{estimator},{p},{mem},{delivery},{t}",
                                n = args.mesh.n,
                                low = args.mesh.low,
                                high = args.mesh.high,
                                q = stem.q,
                                e = stem.epoch.as_secs(),
                                sp = args.multi.message_spacing_secs,
                                m = j + 1,
                                p = s.peer_id,
                                mem = s.member,
                                t = args.adversary_trials,
                            )?;
                        }
                    }
                    let (first, last) = (&rates[0], &rates[rates.len() - 1]);
                    println!(
                        "  k={k} c={c} member after m=1/{M}: first_spy {:.3}/{:.3}, vote {:.3}/{:.3}, disclosure {:.3}/{:.3}",
                        first[&Aggregate::FirstSpy].member,
                        last[&Aggregate::FirstSpy].member,
                        first[&Aggregate::Vote].member,
                        last[&Aggregate::Vote].member,
                        first[&Aggregate::Disclosure].member,
                        last[&Aggregate::Disclosure].member,
                        M = plan.messages,
                    );
                }
            }

            sleep(Duration::from_secs(10)).await
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(seed: u8) -> PeerId {
        PeerId::from_multihash(libp2p::multihash::Multihash::wrap(0x0, &[seed; 32]).unwrap())
            .unwrap()
    }

    fn obs(observer: PeerId, relay: PeerId, ms: u64) -> Observation {
        Observation {
            observer,
            prop_id: relay,
            at: Duration::from_millis(ms),
            stem: false,
        }
    }

    /// The coalition is every peer but the member's PeerIDs, so no sampling
    /// is involved. Relays named in the observations but missing from `peers`
    /// are honest peers outside the coalition.
    fn attack(
        messages: &[(PeerId, Vec<Observation>)],
        member: &[PeerId],
        peers: &[PeerId],
        background: &Background,
    ) -> Vec<HashMap<Aggregate, Success>> {
        let linked: Vec<Linked> = messages
            .iter()
            .map(|(sender, o)| Linked {
                sender: *sender,
                observations: o,
            })
            .collect();
        let mut rng = StdRng::seed_from_u64(1);
        run_multi_attack(
            &linked,
            member,
            peers,
            background,
            peers.len(),
            400,
            &mut rng,
        )
    }

    /// Abstentions do not count, the most frequent guess wins, and a tie
    /// splits evenly rather than by map order.
    #[test]
    fn plurality_takes_the_most_frequent_guess_and_splits_ties() {
        let mut rng = StdRng::seed_from_u64(2);
        assert_eq!(plurality(&HashMap::new(), &mut rng), None);
        let votes = HashMap::from([(pid(1), 3), (pid(2), 2), (pid(3), 1)]);
        assert_eq!(plurality(&votes, &mut rng), Some(pid(1)));
        let tie = HashMap::from([(pid(1), 2), (pid(2), 2)]);
        let ones = (0..10_000)
            .filter(|_| plurality(&tie, &mut rng) == Some(pid(1)))
            .count();
        assert!((4_700..5_300).contains(&ones), "tie split {ones}");
    }

    /// Background shares are pooled over the colluders and kept apart for the
    /// stem and for gossip; peers outside the coalition do not count.
    #[test]
    fn background_shares_are_pooled_per_kind() {
        let (a, b, x, h) = (pid(1), pid(2), pid(3), pid(4));
        let mut bg = Background::default();
        for _ in 0..3 {
            bg.record(a, h, false);
        }
        bg.record(a, x, false);
        bg.record(a, x, true);
        bg.record(b, x, false);
        bg.record(pid(9), h, true);
        assert!((bg.share(&[a], &h, false) - 0.75).abs() < 1e-12);
        assert!((bg.share(&[a, b], &h, false) - 0.6).abs() < 1e-12);
        assert!((bg.share(&[a, b], &x, false) - 0.4).abs() < 1e-12);
        assert_eq!(bg.share(&[a, b], &x, true), 1.0);
        assert_eq!(bg.share(&[a, b], &h, true), 0.0);
        assert_eq!(bg.share(&[pid(8)], &h, false), 0.0);
    }

    /// A message is judged by its stem observations if there are any, and
    /// weighs one in all however many colluders saw it.
    #[test]
    fn evidence_prefers_the_stem_and_weighs_messages_equally() {
        let (source, relay, near) = (pid(1), pid(2), pid(3));
        let colluders: Vec<PeerId> = (10..14).map(pid).collect();
        let first_hop = Observation {
            stem: true,
            ..obs(colluders[0], source, 40)
        };
        let gossip: Vec<Observation> = colluders[1..].iter().map(|c| obs(*c, near, 5)).collect();
        let mut evidence = Evidence::default();
        let mut seen: Vec<&Observation> = gossip.iter().collect();
        seen.push(&first_hop);
        evidence.add(&seen);
        evidence.add(&gossip.iter().collect::<Vec<_>>());
        let split = [obs(colluders[1], relay, 5), obs(colluders[2], near, 6)];
        evidence.add(&split.iter().collect::<Vec<_>>());
        evidence.add(&[]);
        assert_eq!(evidence.shares[&source], 1.0);
        assert_eq!(evidence.shares[&near], 1.5);
        assert_eq!(evidence.shares[&relay], 0.5);
        assert_eq!((evidence.on_stem, evidence.in_gossip), (1, 2));
    }

    /// A hub relays every message to the coalition, the source a third of
    /// them. Shares alone name the hub; against its background share the
    /// source stands out. Colluders are never named, however often they relay.
    #[test]
    fn disclosure_names_the_excess_not_the_hub() {
        let mut rng = StdRng::seed_from_u64(3);
        let (source, hub, colluder) = (pid(1), pid(2), pid(3));
        let evidence = Evidence {
            shares: HashMap::from([(hub, 4.0), (source, 2.0), (colluder, 6.0)]),
            on_stem: 0,
            in_gossip: 6,
        };
        let coalition = HashSet::from([colluder]);
        let background = |p: &PeerId, _| if *p == hub { 0.6 } else { 0.01 };
        assert_eq!(
            evidence.disclose(background, &coalition, &mut rng),
            Some(source)
        );
        assert_eq!(
            evidence.disclose(|_, _| 0.0, &coalition, &mut rng),
            Some(hub)
        );
        let empty = Evidence::default();
        assert_eq!(empty.disclose(|_, _| 0.0, &coalition, &mut rng), None);
    }

    /// Fixed routes: every message reaches the coalition first from the same
    /// relay, so pooling adds nothing and the source is never named.
    /// Redrawn routes: the first hop colludes for two of five messages and
    /// names the source; the wrong guesses all differ, so the vote names the
    /// source from the second message on, while single-message first-spy
    /// stays at the share of messages it caught.
    #[test]
    fn pooling_helps_only_when_routes_change() {
        let source = pid(1);
        let colluders: Vec<PeerId> = (10..15).map(pid).collect();
        let relays: Vec<PeerId> = (20..25).map(pid).collect();
        let mut peers = vec![source];
        peers.extend(&colluders);
        let bg = Background::default();

        let fixed: Vec<(PeerId, Vec<Observation>)> = (0..5)
            .map(|_| (source, vec![obs(colluders[0], relays[0], 10)]))
            .collect();
        let rates = attack(&fixed, &[source], &peers, &bg);
        for per_m in &rates {
            assert_eq!(per_m[&Aggregate::Vote].member, 0.0);
            assert_eq!(per_m[&Aggregate::Disclosure].member, 0.0);
        }

        let first_hop = |j: usize| if j == 0 || j == 3 { source } else { relays[j] };
        let rotating: Vec<(PeerId, Vec<Observation>)> = (0..5)
            .map(|j| (source, vec![obs(colluders[j], first_hop(j), 10)]))
            .collect();
        let rates = attack(&rotating, &[source], &peers, &bg);
        let single: Vec<f64> = rates
            .iter()
            .map(|r| r[&Aggregate::FirstSpy].member)
            .collect();
        assert_eq!(single, vec![1.0, 0.0, 0.0, 1.0, 0.0]);
        // With one observation per message and no background traffic, the
        // disclosure score is the vote count.
        for estimator in [Aggregate::Vote, Aggregate::Disclosure] {
            let pooled: Vec<f64> = rates.iter().map(|r| r[&estimator].member).collect();
            assert_eq!(pooled[0], 1.0, "{estimator}");
            assert!(
                (pooled[1] - 0.5).abs() < 0.1,
                "{estimator}: a tie of two at m=2: {}",
                pooled[1]
            );
            assert!(
                (pooled[2] - 1.0 / 3.0).abs() < 0.1,
                "{estimator}: a tie of three at m=3: {}",
                pooled[2]
            );
            assert_eq!(&pooled[3..], &[1.0, 1.0], "{estimator}");
        }
    }

    /// With two PeerIDs taking turns and each message traced to its sender,
    /// single-message first-spy is right at both levels. The vote after two
    /// messages names one of the two: always the member, the latest sender
    /// half the time.
    #[test]
    fn rotation_is_scored_per_peer_id_and_per_member() {
        let member = [pid(1), pid(2)];
        let colluders: Vec<PeerId> = (10..14).map(pid).collect();
        let mut peers = member.to_vec();
        peers.extend(&colluders);
        let messages: Vec<(PeerId, Vec<Observation>)> = (0..4)
            .map(|j| (member[j % 2], vec![obs(colluders[j], member[j % 2], 10)]))
            .collect();
        let rates = attack(&messages, &member, &peers, &Background::default());
        for per_m in &rates {
            assert_eq!(
                per_m[&Aggregate::FirstSpy],
                Success {
                    peer_id: 1.0,
                    member: 1.0
                }
            );
            assert_eq!(per_m[&Aggregate::Vote].member, 1.0);
        }
        let p = rates[1][&Aggregate::Vote].peer_id;
        assert!((0.35..0.65).contains(&p), "latest sender named {p}");
        // The member's PeerIDs never collude, so here they are the only
        // candidates: a uniform guess always names the member, and the latest
        // sender half the time.
        let r = rates[0][&Aggregate::Random];
        assert_eq!(r.member, 1.0);
        assert!((r.peer_id - 0.5).abs() < 0.1, "random PeerID {}", r.peer_id);
    }

    /// For k PeerIDs, message j is round j / k of PeerID j % k, rounds are
    /// numbered over all K PeerIDs, and publication times rise with j.
    #[test]
    fn plan_spreads_messages_round_robin() {
        let plan = Plan {
            messages: 10,
            peer_ids: 5,
            spacing: Duration::from_secs(5),
        };
        assert_eq!(plan.member_message(0, 1), (0, 0));
        assert_eq!(plan.member_message(3, 1), (0, 15));
        assert_eq!(plan.member_message(4, 3), (1, 6));
        assert_eq!(plan.member_message(9, 5), (4, 9));
        let schedule = plan.schedule();
        assert_eq!(schedule.len(), 50);
        assert!(schedule.windows(2).all(|w| w[0].0 < w[1].0));
        let seqs: HashSet<u32> = schedule.iter().map(|s| s.2).collect();
        assert_eq!(seqs.len(), 50);
        for k in [1, 3, 5] {
            let times: Vec<Duration> = (0..10)
                .map(|j| {
                    let seq = plan.member_message(j, k).1;
                    schedule.iter().find(|s| s.2 == seq).unwrap().0
                })
                .collect();
            assert!(times.windows(2).all(|w| w[0] < w[1]), "k={k}");
        }
    }

    #[test]
    fn regimes_follow_epoch_and_spacing() {
        let stem = |q: f64, epoch: u64| StemConfig {
            q,
            epoch: Duration::from_secs(epoch),
            ..StemConfig::OFF
        };
        let plan = |secs: u64| Plan {
            messages: 10,
            peer_ids: 1,
            spacing: Duration::from_secs(secs),
        };
        let warmup = Duration::from_secs(10);
        assert_eq!(regime(&stem(0.0, 600), &plan(5), warmup), "no_stem");
        assert_eq!(regime(&stem(0.2, 600), &plan(5), warmup), "stem_fixed");
        assert_eq!(regime(&stem(0.2, 1), &plan(31), warmup), "stem_rotating");
        assert_eq!(regime(&stem(0.2, 1), &plan(30), warmup), "stem_mixed");
        assert_eq!(regime(&stem(0.2, 50), &plan(5), warmup), "stem_mixed");
    }
}
