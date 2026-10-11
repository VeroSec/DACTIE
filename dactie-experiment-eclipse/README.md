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
DACTIE disables GossipSub's flood publishing (`flood_publish(false)` in `dactie-peer/src/mybehaviour.rs`), because with it the publisher sends every new message directly to every known subscriber, and any attacker among them learns who the sender is. Flood publishing is, however, one of GossipSub's defenses against eclipse attacks (Vyzovitis et al., *GossipSub: Attack-Resilient Message Propagation in the Filecoin and ETH2.0 Networks*, 2020). This experiment checks whether DACTIE stays available without it. The paper reports only the summary below.

## Setup
Let `H` be the set of honest peers and `A` the set of attacker peers, with `N = H ∪ A`, `H ∩ A = ∅`, `n = |N|` and the attacker fraction `α = |A| / n`.

1. One honest victim peer `V ∈ H` publishes a message every 5 s (`-i fix=5`).
2. Once `--attack-time` has elapsed, every attacker stops forwarding `V`'s messages.
3. After `--stop-time`, `V` sends a measurement message, and each run counts how many of the `|H| - 1` other honest peers received it.

If all of `V`'s mesh peers are attackers, no message from `V` reaches another honest peer, and the eclipse succeeded. A run needs at least two honest peers: `V` and one receiver.

The result folders are named after the number of attackers. The file name `experiment_<min honest>_<max honest>_<stop time>_<interval>.csv` gives the series. Each setting in `output/` was repeated three times:

| Folder | Attackers | Honest peers | Network size `n` | Runs |
| --- | --- | --- | --- | --- |
| `output/<A>/` | 10, 20, …, 100 | 2 to about A/4 | 12–125 | 375 |
| `old_output/<A>/` | 10, 60–100 | 100–120 | 110–220 | 472 |

## Results
Across all 847 runs:

- Every attack failed when attackers made up less than 86 % of the network (`α < 0.86`).
- Every attack failed when at least 10 honest peers remained, regardless of `α`.
- All 41 successful eclipses (`output/` only) had at most 9 honest peers and `α ≥ 0.87`. No run ended in a partial eclipse: either every other honest peer received the message, or none did.

So DACTIE stays available with flood publishing disabled, unless an attacker controls almost the entire network.

## Reproducing the figures
`parse_eclipse_output.py` collects `output/*/ex*.csv` into `eclipse_parsed.csv` and adds `attacker_ratio` and `message_ratio = received_messages / (honest_peers - 1)`. The paper's earlier figure plotted `message_ratio` against `honest_peers` and against `attacker_ratio`.

Note: the glob `ex*.csv` also matches the `experiment_avg_*.csv` files. `eclipse_parsed.csv` therefore mixes single runs with per-setting averages: 759 rows, some of them averages and seemingly also older outputs, against 375 raw runs in `output/`. The counts above use only the raw `experiment_<n>_…csv` files. To rebuild the CSV from raw runs only, change the glob to `experiment_[0-9]*.csv`.

Options added after these runs (`--flood-publish`, `-D/--mesh-n`, `--mesh-n-low`, …) are listed by `--help`.
