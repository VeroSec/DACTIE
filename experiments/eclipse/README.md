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
          Smallest number of honest peers in the series, including the victim [default: 2]
  -N, --n-peers-max <N_PEERS_MAX>
          Largest number of honest peers in the series [default: 20]
  -S, --n-peers-step-size <N_PEERS_STEP_SIZE>
          Step between the numbers of honest peers [default: 1]
  -A, --attack-n <ATTACK_N>
          Number of attacker peers [default: 25]
  -i, --interval <INTERVAL>
          How often peers publish: fix=<secs> or random=<start>,<end> [default: fix=5]
  -s, --stop-time <STOP_TIME>
          How long before measurement message is send [default: 10]
  -a, --attack-time <ATTACK_TIME>
          How long before attackers start to attack [default: 10]
  -r, --repetitions <REPETITIONS>
          How often the experiment should be repeated [default: 5]
  -o, --out-path <OUT_PATH>
          Folder for the series' log, eclipse_<series>.log, which a series run again appends to. It stays local; the results go to --results [default: /tmp/]
      --results <RESULTS>
          CSV file that every run appends one row to, created with a header if missing. Several series, also running in parallel, can share it [default: <out-path>/eclipse_results.csv]
      --flood-publish <FLOOD_PUBLISH>
          Whether the victim floods the message to every subscriber instead of only its mesh. This is the trade-off under test: flood publishing defeats an eclipse but leaks the sender to every peer at once [default: false] [possible values: true, false]
  -D, --mesh-n <MESH_N>
          Gossipsub mesh degree D, matching the anonymity experiment so the two halves of the trade-off are measured at the same configuration [default: 12]
      --mesh-n-low <MESH_N_LOW>
          Lower edge of the mesh band. Defaults to D-2
      --mesh-n-high <MESH_N_HIGH>
          Upper edge of the mesh band. Defaults to 2*D
      --gossip-lazy <GOSSIP_LAZY>
          Gossip emission: peers announced per heartbeat at least (libp2p's default, as in the DACTIE nodes). With --gossip-factor 0 as well, emission is off [default: 6]
      --gossip-factor <GOSSIP_FACTOR>
          Gossip emission: share of non-mesh peers announced per heartbeat (libp2p's default, as in the DACTIE nodes) [default: 0.25]
      --bootstrap-port <BOOTSTRAP_PORT>
          TCP port the bootstrap peer binds. Give consecutive invocations different ports: the previous run's peers may still hold the socket, and the bind fails with AddrInUse before a single run has started [default: 10001]
  -h, --help
          Print help
  -V, --version
          Print version
```

## Output
Every run appends one row to the results CSV (`--results`, by default `<out-path>/eclipse_results.csv`) as soon as it ends, so an interrupted series keeps its finished runs. The file is created with its header if it does not exist yet; several series, also running in parallel, can append to the same file. Columns:

- the network: `total_peers`, `honest_peers` (including the victim), `attacker_peers`, `attacker_ratio = attacker_peers / total_peers`;
- the configuration: `mesh_n_low`, `mesh_n`, `mesh_n_high`, `flood_publish`, `gossip_lazy`, `gossip_factor`, `attack_time`, `stop_time`, `interval`, and `run`, the repetition;
- the outcome: `received_messages`, the number of other honest peers that received the victim's measurement message, and `message_ratio = received_messages / (honest_peers - 1)`.

Everything the harness prints also goes to the series' log, `<out-path>/eclipse_<series>.log`. The log stays local: keep `--out-path` outside the repository (`results/eclipse/` ignores `*.log`) and publish only the CSV.

## Example
Here is an example where only six honest peers are in the network, and the rest are attackers, so an eclipse is likely to succeed. After 15 seconds the attackers stop forwarding the victim's messages; 30 seconds after the start, the victim sends the measurement message.
```
cargo run --release -p dactie-experiment-eclipse -- -n 6 -N 6 -A 60 -a 15 -s 30 -r 20 -o /tmp/eclipse/
python3 scripts/parse_eclipse_rerun.py /tmp/eclipse/eclipse_results.csv
```

## Why this experiment exists
DACTIE disables GossipSub's flood publishing (`flood_publish(false)` in `crates/dactie-peer/src/mybehaviour.rs`), because with it the publisher sends every new message directly to every known subscriber, and any attacker among them learns who the sender is. Flood publishing is, however, one of GossipSub's defenses against eclipse attacks (Vyzovitis et al., *GossipSub: Attack-Resilient Message Propagation in the Filecoin and ETH2.0 Networks*, 2020). This experiment checks whether DACTIE stays available without it, and whether gossip emission matters for that. The paper reports only the summary below.

## Setup
Let `H` be the set of honest peers and `A` the set of attacker peers, with `N = H ∪ A`, `H ∩ A = ∅`, `n = |N|` and the attacker fraction `α = |A| / n`.

1. One honest victim peer `V ∈ H` publishes a message every 5 s (`-i fix=5`).
2. Once `--attack-time` has elapsed, every attacker stops forwarding `V`'s messages.
3. After `--stop-time`, `V` sends a measurement message, and each run counts how many of the `|H| - 1` other honest peers received it.

If all of `V`'s mesh peers are attackers, no message from `V` reaches another honest peer, and the eclipse succeeded. A run needs at least two honest peers: `V` and one receiver.

All peers of a run share one process and connect over loopback TCP. The defaults are the configuration of the release: mesh band 10/12/24, flood publishing off, and libp2p's default gossip emission.

## Results with the released configuration (2026-11-15/16)
The paper's figure. Every peer uses the mesh band 10/12/24 and no flood publishing (both defaults), and gossip emission either as in the release (`--gossip-lazy 6 --gossip-factor 0.25`, libp2p's defaults) or off (`--gossip-lazy 0 --gossip-factor 0`). The parsed runs are in `results/eclipse/rerun-2026-11-15/`: `eclipse_gossip_parsed.csv`, one row per run, split by setting into `eclipse_gossip_on.csv` and `eclipse_gossip_off.csv`. This rerun predates the results CSV: the harness wrote one CSV per series, into `gossip-<on|off>/output/<A>/` (small grid) or `gossip-<on|off>/old_output/<A>/` (large grid). Every run publishes the measurement message after 180 s and takes about 233 s.

| Grid | Attackers `A` | Honest peers | `n` | Runs | Flags per series |
| --- | --- | --- | --- | --- | --- |
| small | 10, 20, …, 100 | 2 to ⌊A/4⌋ | 12–125 | 375 | `-n 2 -N <⌊A/4⌋> -A <A> -s 180 -i fix=5 -r 3` |
| large | 10, 30, 40, …, 100 | 100–120 | 110–220 | 945 | `-n 100 -N 120 -A <A> -s 180 -i fix=5 -r 5` |

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
`python3 scripts/parse_eclipse_rerun.py <input> [<out-dir>]` (from the repository root) splits runs by gossip setting into `eclipse_gossip_on.csv` and `eclipse_gossip_off.csv`, the two series of the paper's figure (filled and hollow), and prints the summary above. `<input>` is a results CSV, or for this rerun the folder with its per-series CSVs (kept on labrada), from which it also writes `eclipse_gossip_parsed.csv` with every run; that is how the files in `results/eclipse/rerun-2026-11-15/` were made. The figure plots `message_ratio` against `honest_peers` and against `attacker_ratio`.

## Earlier series (847 runs, camera-ready paper)
The camera-ready paper reported 847 runs of an earlier harness version (before `b5f7ea7`). It set only `flood_publish(false)`, so its peers used libp2p's default mesh band 5/6/12 instead of the release's 10/12/24. Every attack failed when attackers made up less than 86 % of the network or when at least 10 honest peers remained, and 41 runs of the small grid ended in an eclipse. With the release's mesh band, the same small grid gives 21 eclipses, and the attacker share below which every attack failed rises to 10/11. The likely reason is the mesh size: an eclipse needs every one of the victim's mesh peers to be an attacker, and the release's mesh is twice as large. Their parsed runs are in `results/eclipse/eclipse_parsed.csv`, one row per run with the source folder as `series`; the raw output and its parser are in the repository's history up to `f0e0ad3`.
