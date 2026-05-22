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