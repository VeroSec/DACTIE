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