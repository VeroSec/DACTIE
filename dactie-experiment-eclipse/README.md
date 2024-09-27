# Anonymity Experiment
This experiment can be used to test the source anonymity of a specific GossipSub Configuration.
It starts an experiment series that automatically increases the total peer number. The experiment output is saved to a csv file.
The number of peers that received from the publisher, from other peers and the number of unique propagation peers is outputted.

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
          How often Peers Should publish. Input can be fix=<secs> or random=>start,end> [default: fix=5]
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