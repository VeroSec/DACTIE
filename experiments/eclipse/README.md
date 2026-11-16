# Eclipse Experiment
This experiment can be used to test if under which conditions attackers can perform an eclipse attack.
The attacker peers start to ignore all messages from the victim peer, after attack-timer has elapsed.
At the end of the experiment the number of honest peers that received a message from the victim is reported.
If an eclipse attack is successful, this number will be zero or at least lower than the number of honest receiver peers.


## Configuration Parameters
```
Usage: dactie-experiment-eclipse [OPTIONS]

Options:
  -n, --n-peers-min <N_PEERS_MIN>
          Min number of peers in series [default: 2]
  -N, --n-peers-max <N_PEERS_MAX>
          Max number of peers in series [default: 20]
  -S, --n-peers-step-size <N_PEERS_STEP_SIZE>
          Step Size of Series [default: 1]
  -A, --attack-n <ATTACK_N>
          Number of attacker peers [default: 25]
  -i, --interval <INTERVAL>
          How often Peers Should publish. Input can be fix=<secs> or random=>start,end> [default: fix=5]
  -s, --stop-time <STOP_TIME>
          How long before measurement message is send [default: 10]
  -a, --attack-time <ATTACK_TIME>
          How long before attackers start to attack [default: 10]
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
Here is an example where only two honest nodes are in the network, and the rest consists of malicious peers. Therefore, an eclipse attack is very likely to succeed.
After 30 seconds the attackers start to ignore the victims peers messages. 45 seconds after the experiment a measurement message is send trough the network.
The recipients of this message report if they got it or not.
```
cargo run --package dactie-experiment-eclipse --bin dactie-experiment-eclipse -- -n 6 -N 6 -A 60 -a 15 -s 30 -r 20 
```
## Why this experiment exists
DACTIE disables GossipSub's flood publishing (`flood_publish(false)` in `crates/dactie-peer/src/mybehaviour.rs`), because with it the publisher sends every new message directly to every known subscriber, and any attacker among them learns who the sender is. Flood publishing is, however, one of GossipSub's defenses against eclipse attacks (Vyzovitis et al., *GossipSub: Attack-Resilient Message Propagation in the Filecoin and ETH2.0 Networks*, 2020). This experiment checks whether DACTIE stays available without it, and whether gossip emission matters for that. The paper reports only the summary below.

## Setup
Let `H` be the set of honest peers and `A` the set of attacker peers, with `N = H ∪ A`, `H ∩ A = ∅`, `n = |N|` and the attacker fraction `α = |A| / n`.

1. One honest victim peer `V ∈ H` publishes a message every 5 s (`-i fix=5`).
2. Once `--attack-time` has elapsed, every attacker stops forwarding `V`'s messages.
3. After `--stop-time`, `V` sends a measurement message, and each run counts how many of the `|H| - 1` other honest peers received it.

If all of `V`'s mesh peers are attackers, no message from `V` reaches another honest peer, and the eclipse succeeded. A run needs at least two honest peers: `V` and one receiver.

All peers of a run share one process and connect over loopback TCP. Options added after the first series (`--flood-publish`, `-D/--mesh-n`, `--mesh-n-low`, `--mesh-n-high`, `--gossip-lazy`, `--gossip-factor`, `--bootstrap-port`) are listed by `--help`; the defaults are the configuration of the release (mesh band 10/12/24, flood publishing off, libp2p's default gossip emission).

## Results with the released configuration (2026-11-15/16)
The paper's figure. Every peer uses the mesh band 10/12/24 and no flood publishing (both defaults), and gossip emission either as in the release (`--gossip-lazy 6 --gossip-factor 0.25`, libp2p's defaults) or off (`--gossip-lazy 0 --gossip-factor 0`). The data is in `results/eclipse/rerun-2026-11-15/`, one folder per setting (`gossip-on/`, `gossip-off/`) and per number of attackers `A`. Every run publishes the measurement message after 180 s and takes about 233 s.

| Grid | Folder | Attackers `A` | Honest peers | `n` | Runs | Flags per series |
| --- | --- | --- | --- | --- | --- | --- |
| small | `output/<A>/` | 10, 20, …, 100 | 2 to ⌊A/4⌋ | 12–125 | 375 | `-n 2 -N <⌊A/4⌋> -A <A> -s 180 -i fix=5 -r 3` |
| large | `old_output/<A>/` | 10, 30, 40, …, 100 | 100–120 | 110–220 | 945 | `-n 100 -N 120 -A <A> -s 180 -i fix=5 -r 5` |

| Gossip emission | Grid | Victim cut off | Partly delivered |
| --- | --- | --- | --- |
| on (release) | small | 21 of 375: 2–8 honest peers, `α ≥ 10/11` | 0 |
| on (release) | large | 0 of 945 | 0 |
| off | small | 102 of 375: 2–19 honest peers, `α ≥ 0.80` | 0 |
| off | large | 0 of 945 | 0 |

- With the release configuration, every attack failed when attackers made up less than 10/11 of the network (fewer than ten per honest peer) or when at least 9 honest peers remained.
- Turning gossip emission off makes the same attacks succeed about five times as often in small networks, and with up to 19 honest peers and 80 % attackers: gossip repairs what the mesh fails to deliver. It brings no measurable anonymity gain, since with emission on the source never reached more than its mesh peers directly in the anonymity runs (`results/sizes/`). The release therefore keeps gossip emission on.
- No run ended in a partial eclipse: either every other honest peer received the message, or none did.
- Neither grid has runs with 50 % to 80 % attackers: the small grid starts at 80 % (`⌊A/4⌋` honest peers), the large one ends at 50 % (100 attackers, 100 honest peers). The thresholds above are those of the measured runs.

## Running many series on one host
Each run connects its peers roughly all to all over loopback, and Linux tracks every such connection in its conntrack table. When the table is full, the kernel drops packets silently; in the results this looks like cut-off victims with fewer attackers than mesh peers, or partial deliveries. A first rerun with up to 20 processes at once on labrada (`nf_conntrack_max` 262144) was discarded for that reason. Before trusting a run, check the kernel log for its time window:

```bash
journalctl -k --since "<start>" | grep -c "table full"
```

A process with `n` peers peaked at up to about `2.1 n²` entries (at run boundaries, when the connections of one run are in TIME_WAIT while the next run connects). Start processes only while the sum of these estimates fits below `nf_conntrack_max`, and give each one its own `--bootstrap-port`.

## Reproducing the figure
`python3 scripts/parse_eclipse_rerun.py` (from the repository root) collects `results/eclipse/rerun-2026-11-15/` into `eclipse_gossip_parsed.csv`, one row per run (2,640 rows), and splits it into `eclipse_gossip_on.csv` and `eclipse_gossip_off.csv`, the two series of the paper's figure (filled and hollow). It adds `attacker_ratio` and `message_ratio = received_messages / (honest_peers - 1)`, and prints the summary above. The figure plots `message_ratio` against `honest_peers` and against `attacker_ratio`.

## Earlier series (847 runs, superseded)
Before the rerun, the paper reported 847 runs of an earlier harness version (before `b5f7ea7`). It set only `flood_publish(false)`, so its peers used libp2p's default mesh band 5/6/12 instead of the release's 10/12/24, and libp2p's default gossip emission; the repository does not record the exact command lines. They are in `results/eclipse/`, named `experiment_<min honest>_<max honest>_<stop time>_<interval>.csv`:

| Folder | Attackers | Honest peers | Network size `n` | Runs |
| --- | --- | --- | --- | --- |
| `results/eclipse/output/<A>/` | 10, 20, …, 100 | 2 to about A/4 | 12–125 | 375 |
| `results/eclipse/old_output/<A>/` | 10, 60–100 | 100–120 | 110–220 | 472 |

Every attack failed when attackers made up less than 86 % of the network or when at least 10 honest peers remained; all 41 successful eclipses (`output/` only) had at most 9 honest peers and `α ≥ 0.87`, and none was partial. With the release's mesh band, the same small grid gives 21 instead of 41 eclipses, the attacker share below which every attack failed rises from 86 % to 10/11 (90.9 %), and the number of honest peers from which every attack failed falls from 10 to 9. The likely reason is the mesh size: an eclipse needs every one of the victim's mesh peers to be an attacker, and the release's mesh is twice as large. `python3 scripts/parse_eclipse_output.py` collects them into `results/eclipse/eclipse_parsed.csv` (847 rows, with the source folder as `series`).
