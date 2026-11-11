# Anonymity Experiment
This experiment can be used to test the source anonymity of a specific GossipSub Configuration.
It starts an experiment series that automatically increases the total peer number. The experiment output is saved to a csv file.
The number of peers that received from the publisher, from other peers and the number of unique propagation peers is outputted.

## How peers connect
All peers run in one process and listen on 127.0.0.1. Each peer registers its address in a shared directory
and, while it holds fewer than `--conn-low` connections (default 50), dials a uniform random sample of the
other peers. A peer holding `--conn-high` connections (default 100) refuses new ones before the Noise
handshake, and every 5 s it closes random connections that are older than 20 s and outside its GossipSub
mesh, down to `--conn-low`. The connection graph is thus a random graph with 50 to 100 connections per
peer; there is no Kademlia DHT. Peers start in waves of `--spawn-per-sec` (default 50).

Every link has a simulated one-way delay. Each peer sits at a random point of a unit square, and the
delay grows with the distance between the two ends, from 10 ms for neighbours to 100 ms for opposite
corners (`--link-latency-ms 10,100`, the default; `0` turns it off). Delays are the same in both
directions and obey the triangle inequality, so a detour via a third peer is never faster than the
direct link. Without the simulation all links of the single process have zero delay, CPU scheduling
decides which copy of a message arrives first, and relayed copies often beat the source's direct one,
so peers record fewer direct receipts than in a real network. With the simulation, TCP runs with
Nagle's algorithm off, since loopback acknowledgements do not see the simulated delay. At 100 peers
the source's mesh peers received 70% of their first copies directly without the simulation and 100%
with it.

## Repeated messages and PeerID rotation
`--messages-per-source M` switches to multi-message runs (`src/multi_message.rs`): the source member
publishes M measurement messages per run, and a coalition that can link them to one unknown sender
(same style, same topic) pools its evidence over them. The member spreads its messages round robin
over k PeerIDs (`--peer-ids-per-source`, e.g. `1,3,5`; DACTIE allows at most 5 per registration
epoch). One run serves every k: K PeerIDs, K the largest k, each publish M messages
`--message-spacing-secs` apart, taking turns within a round, and for a given k the member owns the
first k of them and its message j is round j / k of PeerID j % k. The first PeerID is the peer started
last, the source of a single-message run; the others are random peers.

After each m = 1..M messages the coalition names the sender with
- `first_spy`: first-spy on message m alone, i.e. no pooling;
- `first_spy_vote`: the first-spy guess named most often over messages 1..m (ties uniform);
- `disclosure`: statistical disclosure over messages 1..m. Each message counts the relays its
  observations name, by the stem observations if a colluder sat on the stem, else by the gossip ones
  (as first-spy does), with one unit of weight per message shared among them. The guess is the peer
  whose summed shares most exceed its share of the coalition's background receipts of the same kind;
  peers report their background receipts in this mode for that baseline, so a peer that relays much
  of everyone's traffic to the coalition is not named for it. Colluders are never named;
- `random`: a uniform guess among the peers outside the coalition.

Coalitions are drawn from the peers that are not the member's PeerIDs. `success_member` counts a
guess that is any of the member's PeerIDs, `success_peer_id` only the PeerID that sent message m.

The stem regime follows from the flags and is written to every row:

| `regime` | Condition | Meaning |
| --- | --- | --- |
| `no_stem` | `--stem-fluff-probability 0` | |
| `stem_fixed` | `--time` + M × spacing ≤ `--stem-epoch-secs` | every peer keeps its successors, role and routes for all M messages |
| `stem_rotating` | spacing ≥ `--stem-epoch-secs` + 30 s | every peer draws new successors and a new role between two messages of a PeerID (30 s is the most epoch jitter) |
| `stem_mixed` | anything else | a warning is printed |

The output is `multi_message_<n_min>_<n_max>_<time>_D<D>_<interval>_q<q>_e<epoch>_s<spacing>_M<M>.csv`
in `--out-path`, with one row per run, k, coalition size, m and estimator:
```
total_peers,mesh_n,mesh_n_low,mesh_n_high,stem_q,stem_epoch_secs,spacing_secs,regime,run,k,coalition_size,m,estimator,success_peer_id,success_member,delivery_min,trials
```
`delivery_min` is the smallest share of the other peers that received a measurement message of the
run; filter on it as on `delivery_ratio`. Coalition sizes come from `--coalition-sizes` and
`--adversary-fractions`, with `--adversary-trials` sampled coalitions each, as in single-message runs.
The three regimes at 100 peers:
```
B=target/release/dactie-experiments
common="-n 100 -N 100 -r 2 -D 12 --mesh-n-high 24 --messages-per-source 10 --peer-ids-per-source 1,3,5 --coalition-sizes 1,10 --adversary-fractions 0.05"
$B $common --message-spacing-secs 5 -o out/no_stem/
$B $common --stem-fluff-probability 0.2 --message-spacing-secs 5 -o out/stem_fixed/
$B $common --stem-fluff-probability 0.2 --stem-epoch-secs 1 --message-spacing-secs 31 -o out/stem_rotating/
```
A run takes the start-up and `--time`, M × spacing of publishing, and 40 s of drain and pause.

## Timing estimators
`adversary_*.csv` scores every estimator per run and coalition size (column `estimator`). `random`,
`observed_set`, `first_spy` (stem-aware) and `plurality` use only what the colluders saw. Three more
model a worst-case insider coalition that also knows every peer's GossipSub mesh at the measurement and
the delay of every link (`src/timing.rs`; the spy-based adversary of Fanti and Viswanath 2017, after
Pinto, Thiran and Vetterli 2012):

- `timing_delay`: for each candidate source s, colluder j should receive the message at t0 + d(s, j),
  the fastest mesh path with the true link delays plus a processing constant per hop. The publish time
  t0 is fitted as the mean residual; the candidate with the smallest sum of squared residuals is named,
  ties uniformly.
- `timing_hops`: the same with hop counts times a hop time (topology known, delays unknown).
- `timing_delay_stem`: the stem-aware first-spy rule if a colluder received the message on the stem,
  `timing_delay` otherwise. With the stem on, the first two find the diffuser rather than the source.

Only each colluder's earliest GossipSub receipt enters the fit. The processing constant and the hop
time are the medians of t_j - t_i - w(i, j) and t_j - t_i over the run's relay steps (peer i relayed
peer j's first copy); the log line `Timing model:` reports them and how well the model explains the
trace from the true source. With a single colluder every candidate fits perfectly, so at c = 1 the
timing estimators sit at the random baseline. Simulated link delays have no jitter, so the timing
estimators work with exact delays: an upper bound on what timing reveals.

## Configuration Parameters
```
Usage: dactie-experiments [OPTIONS]

Options:
  -n, --n-peers-min <N_PEERS_MIN>
          Min number of peers in series [default: 2]
  -N, --n-peers-max <N_PEERS_MAX>
          Max number of peers in series [default: 20]
  -S, --n-peers-step-size <N_PEERS_STEP_SIZE>
          Step Size of Series [default: 1]
  -i, --interval <INTERVAL>
          Background traffic. rate=<msgs/s> sets a network-wide rate (each peer publishes every N/rate
          seconds, starting at a random point); fix=<secs> or random=<start,end> set a per-peer
          interval instead [default: rate=40]
  -t, --time <TIME>
          How long before measurement message is send [default: 10]
  -r, --repetitions <REPETITIONS>
          How often the experiment should be repeated [default: 5]
  -o, --out-path <OUT_PATH>
          Folder where the result should be saved [default: /tmp/]
  -h, --help
          Print help
  -V, --version
          Print version

```
## Example
```
cargo run --package dactie-experiments --bin dactie-experiments -- -r 5 -t 60 -i fix=3 -n 85 -N 100 -S 5
```