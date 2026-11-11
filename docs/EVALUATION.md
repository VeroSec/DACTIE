# Evaluation

How every result of the paper was produced, how to run it again, and where its data is. The
data files and their columns are described in [results/README.md](../results/README.md).

## Overview

| Result | Where it runs | Run | Summarize | Data |
| --- | --- | --- | --- | --- |
| End-to-end latency, MISP and STIX | testbed, four hosts | `scripts/latency_sweep.sh` | `scripts/latency_raw_csv.py`, `scripts/latency_summary.py` | `results/latency/` |
| Soak test, 33.4 hours | testbed | `scripts/soak_sampler.sh` | – | `results/latency/soak/` |
| Archive queries | testbed | `BENCH_ARCHIVE_*` settings | – | numbers below |
| Bytes on the wire | one host | `scripts/capture_on_wire.sh` | `scripts/analyze_pcap.sh` | – |
| Signing, verifying, opening | one CPU core | `cargo bench -p dactie-benchmark` | – | `experiments/benchmark/README.md` |
| Single-attacker bound | simulation | `scripts/anonymity_sizes.sh` | `scripts/size_figure_data.py` | `results/sizes/`, `results/anonymity_parsed.csv` |
| Coalition first-spy attack | simulation | `scripts/anonymity_coalition.sh OUT 0` | `scripts/coalition_figure_data.py` | `results/coalition/` |
| Stem phase | simulation | `scripts/anonymity_coalition.sh OUT 0.2 0.1 0.5` | `scripts/coalition_figure_data.py` | `results/coalition/stem/` |
| Timing estimators | simulation | `scripts/anonymity_coalition.sh OUT 0 0.2` | `scripts/repeated_timing_summary.py` | `results/coalition/timing/` |
| Repeated messages, PeerID rotation | simulation | `scripts/anonymity_repeated.sh` | `scripts/repeated_timing_summary.py` | `results/repeated/` |
| Cost of the source's loop re-send | model | `scripts/stem_loop_cost.py 100 500` | – | [DANDELION.md](DANDELION.md) |
| Eclipse attacks | simulation | `experiments/eclipse` | `scripts/parse_eclipse_output.py` | `results/eclipse/` |

The payloads of the latency runs are in `workloads/`: a MISP event from the abuse.ch
MalwareBazaar feed (`workloads/misp/`) and STIX 2.1 bundles from the MITRE ATT&CK ICS domain
(`workloads/stix/`, provenance and licence in `workloads/stix/NOTICE.md`), each cut at object
boundaries to 1, 4, 16, 64 and 256 KiB, plus the complete event (2.4 MB) and feed (3.0 MB).
`scripts/make_stix_payloads.py` rebuilds the STIX bundles from the upstream feed.

## Testbed

Four hosts on a switched LAN, set up with [RUNBOOK.md](RUNBOOK.md) (Phases 2–8, from wiped
state). Clocks agree within 0.06 ms (chrony).

| Host | Services | Ports |
| --- | --- | --- |
| A (`.116`) | Authorization Authority | 10001 |
| B (`.117`) | PostgreSQL, Archive 1, `peer_1` | 5432, 20011, 21011 |
| C (`.118`) | PostgreSQL, Archive 2, `peer_2` | 5433, 20012, 21012 |
| D (`.119`) | `peer_3`, `peer_4` | 21013, 21014 |

`peer_4` publishes in every run. GossipSub does not deliver a publisher its own message, so
`peer_4` has no receive latency and appears in no table below.

The peer image is built with the `bench` feature (`./build.sh`), which compiles in the
benchmark publisher. `BENCH_PUBLISH`, `BENCH_PAYLOAD_FILE` (a path inside the image, e.g.
`/usr/src/dactie/workloads/misp/<event>.json`) and `BENCH_RATE` configure it. Receivers log
one `BENCH_RX` line per delivery, with the forwarding neighbour, the publisher's sequence
number and the latency.

## End-to-end latency

The publisher sends at 40 messages per second, the rate Huff and Li estimate for critical
infrastructure operators, and the complete event and feed at 5 messages per second, since
40 per second of 2.9 MB exceed the link. Every message is MLS-encrypted, signed with the group
signature and disseminated via GossipSub. Latency runs from publication to delivery to the
application at a peer on another host. Each row is the mean / p95 in ms over 1,000 deliveries
at `peer_1`, after dropping the first 2 s, with the publisher outside the stem phase.

The paper's table, measured on 2026-10-26 with the released code, first with Nagle's algorithm
off (DACTIE's default), then on (`TCP_NODELAY=false`, libp2p's default):

| Payload | MISP, Nagle off | STIX, Nagle off | MISP, Nagle on | STIX, Nagle on |
| --- | --- | --- | --- | --- |
| 1 KiB | 19.1 / 27 | 19.2 / 27 | 20.9 / 34 | 22.0 / 36 |
| 4 KiB | 19.5 / 27 | 19.5 / 28 | 19.4 / 27 | 19.2 / 27 |
| 16 KiB | 20.4 / 28 | 20.3 / 28 | 20.1 / 28 | 19.6 / 28 |
| 64 KiB | 21.9 / 31 | 21.6 / 30 | 22.0 / 31 | 21.9 / 30 |
| 256 KiB | 25.5 / 33 | 25.3 / 33 | 25.6 / 35 | 25.5 / 34 |
| Complete event / feed | 104.4 / 118 | 117.5 / 134 | 102.0 / 115 | 116.7 / 132 |

Latency stays flat across a 256-fold increase in payload size, and MISP and STIX are
indistinguishable at every size: per-message work, not transport, dominates the cost.
`peer_2` (19.1–25.6 ms) and `peer_3` (18.3–25.5 ms) agree with `peer_1` for 1–256 KiB.

To run the whole table again from a machine with ssh access to the four hosts (about
45 minutes):

```bash
scripts/latency_sweep.sh OUT_DIR                                     # raw BENCH_RX lines in OUT_DIR/raw/
python3 scripts/latency_raw_csv.py OUT_DIR/raw OUT_DIR/raw.csv        # one row per receipt
python3 scripts/latency_summary.py OUT_DIR/raw.csv OUT_DIR/summary.csv  # mean / p95 per run and receiver
```

`results/latency/raw-2026-10-26.csv` is the paper's sweep in that format, and
`results/latency/rerun-2026-10-26.csv` its summary per run and receiver; the last two steps
regenerate both byte for byte from the sweep's raw lines.

### Earlier measurements on the same testbed

- **2026-08:** 21.0 to 26.5 ms for 1–256 KiB and 89.8 / 102.2 ms for the complete event and
  feed, with Nagle on. Its raw logs were not kept, which is why the paper uses the run of
  2026-10-26. On-wire sizes from this run: 1 KiB of MISP is 1,169 bytes on the wire,
  256 KiB are 262,431 bytes, the complete event 2,362,522 bytes and the complete feed
  2,951,973 bytes.
- **2026-10-22** (`results/latency/rerun-2026-10-22.csv`), a build from before the release,
  also compared Nagle on and off on the same day. Nagle does not delay deliveries that come
  straight from the publisher, but relayed ones: `peer_3` gets about half its copies via
  another peer and took 25.9 instead of 19.3 ms with Nagle on (16 KiB). With the stem on at the
  publisher (`STEM_Q=0.2`), 16 KiB events took 65 to 80 ms on average in this four-peer
  network, against about 20 ms without it.

### One more hop costs about 10 ms

In the soak test below, `peer_3` averaged 25.6 ms, 3.6 ms above the other receivers, with a
lower spread. Splitting each receiver's receipts by whether they came straight from the
publisher or via another peer (9,999 receipts each):

| Receiver | Direct share | Direct mean | Relayed share | Relayed mean | Extra hop |
| --- | --- | --- | --- | --- | --- |
| `peer_1` | 99.5 % | 21.82 ms | 0.5 % | 31.70 ms | 9.88 ms |
| `peer_2` | 99.6 % | 21.57 ms | 0.4 % | 31.57 ms | 10.00 ms |
| `peer_3` | 52.0 % | 21.25 ms | 48.0 % | 30.87 ms | 9.62 ms |

Direct and relayed latency are the same on all hosts. `peer_3`'s higher mean comes from its
even mix of both paths, not from its host, although it shares that host with the publisher.

## Soak test

16 KiB MISP events at 40 messages per second for 33.4 hours (2026-08-22 12:41 to
2026-08-23 22:04 UTC), with Nagle on. Every 62 s, `scripts/soak_sampler.sh` summarized the
last 60 s of each receiver's log, 1,925 samples per receiver:

| Receiver | Mean | SD | First 10 min | Last 10 min | Messages / min | Memory, start → end | Minutes without delivery |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `peer_1` (host B) | 21.97 ms | 0.36 | 22.25 | 21.90 | 2,420 | 11 → 13 MB | 0 of 1,925 |
| `peer_2` (host C) | 21.87 ms | 0.39 | 22.63 | 21.62 | 2,420 | 13 → 15 MB | 0 of 1,925 |
| `peer_3` (host D) | 25.56 ms | 0.11 | 25.62 | 25.61 | 2,420 | 12 → 12 MB | 0 of 1,926 |

Over 13,979,188 sampled deliveries, latency did not drift, memory grew by at most 2 MB, and
no sampled minute was without delivery. Receiving 2,420 messages per minute against 2,400
published is consistent with complete delivery; messages were not counted one by one, so this
shows no sustained shortfall, not zero loss. Data: `results/latency/soak/`.

To run it: start the publisher with `BENCH_PUBLISH=true` and a 16 KiB payload, then on each
receiver host

```bash
scripts/soak_sampler.sh <compose profile> <service> out.csv
```

## Archive queries

A peer asks an archive for the group commits and proposals newer than its epoch
(`update_groups`), and the archive answers from PostgreSQL. Measured across hosts at two
queries per second with the build of 2026-08:

| Condition | n | Mean | p50 | p95 | Max |
| --- | --- | --- | --- | --- | --- |
| Quiet network | 82 | 105.1 ms | 106 | 110 | 111 |
| Under 40 msg/s of broadcasts | 100 | 108.6 ms | 104 | 151 | 172 |

The queries returned no commits, so they measure the round trip, not the transfer of a
backlog. Signature verification does not explain the 105 ms: one verification takes 7.53 ms
on an AMD EPYC 7763 and 4.97 ms on an Intel Core Ultra 7 165U, and an empty query verifies at
most one signature. The rest is the database round trip and request handling, which have not
been profiled. A peer needs a private group to query at all: `update_groups` skips the common
group.

To run it: `BENCH_ARCHIVE_QUERY=true BENCH_ARCHIVE_PEER=<archive PeerID> BENCH_ARCHIVE_RATE=2`
on a peer.

## Two changes the measurements required

- **GossipSub's maximum message size.** The default of 65,536 bytes silently dropped every
  payload from 64 KiB up (a 64 KiB MISP event is 65,811 bytes on the wire). All nodes now
  accept up to 8 MiB; they must agree, or a receiver drops the message without an error.
- **Signature encoding.** The compact byte encoding instead of `amcl_wrapper`'s hex strings
  takes a signature from 614 to 226 bytes on the wire. All nodes must run a matching build.

## Signature microbenchmark

`cargo bench -p dactie-benchmark --bench thressig` measures signing, verifying, joining and
opening on one CPU core, in about 5 minutes. Setup and results are in
[experiments/benchmark/README.md](../experiments/benchmark/README.md).

## Anonymity simulations

The anonymity harness (`experiments/anonymity`, package `dactie-experiments`) runs all peers of a network in one
process, over simulated links of 10 to 100 ms by distance between peers, with 50 to 100
connections per peer and GossipSub's mesh band 10/12/24 unless set otherwise (details in its
[README](../experiments/anonymity/README.md)). Every run publishes one measurement message
(ten in the repeated-message runs) and records who received it from whom and when; the
attacks then sample coalitions among the peers and try to name the source.

The paper's series ran on one server with 64 vCPUs (AMD EPYC 7763), 125 GiB of memory and
Linux 6.8. Each script below builds the harness, writes one folder per configuration with a
`DONE` marker and a `meta.txt` log, and resumes where it stopped when run again.

| Series | Command | Time on that server |
| --- | --- | --- |
| Single attacker, 50 to 850 peers | `scripts/anonymity_sizes.sh OUT` | about 7 hours |
| Coalition grid without the stem | `scripts/anonymity_coalition.sh OUT 0` | about 3 hours |
| Coalition grid with the stem | `scripts/anonymity_coalition.sh OUT 0.2 0.1 0.5` | about 3 hours per value of q |
| Repeated messages | `scripts/anonymity_repeated.sh OUT` | about 4 hours |

The coalition script also scores the timing estimators, so `results/coalition/timing/`
corresponds to `scripts/anonymity_coalition.sh OUT 0 0.2`; the paper's coalition and stem grids
ran before the timing estimators existed. The coalitions are sampled with a fixed seed
(`--adversary-seed 2`), but message propagation depends on thread scheduling, so a rerun gives
close, not identical numbers. Then:

```bash
python3 scripts/size_figure_data.py anonymity_parsed.csv OUT                         # sizes
python3 scripts/coalition_figure_data.py coalition_parsed.csv OUT/q0/D*_N*         # Figure 2
python3 scripts/coalition_figure_data.py stem_q0.2.csv OUT/q0.2/D*_N*              # stem, q = 0.2
python3 scripts/repeated_timing_summary.py results                                 # timing, repeated
```

The summaries of the paper's data regenerate the committed CSVs; [results/README.md](../results/README.md)
lists the results per series.

`scripts/stem_loop_cost.py 100 500` replays the stem routing on 100 random graphs with 500
messages each and gives the cost of the source's re-send when its message loops back (1.5 % at
200 peers, see [DANDELION.md](DANDELION.md)).

## Eclipse attacks

The eclipse harness (`experiments/eclipse`) lets attackers stop forwarding a victim's
messages after a set time and counts how many honest peers still receive the victim's
measurement message, without flood publishing. Its
[README](../experiments/eclipse/README.md) describes the setup and the two series of the
paper (847 runs, 12 to 220 peers). The repository does not record the exact commands of those
series; one run of the kind they contain is

```bash
cargo run --release -p dactie-experiment-eclipse -- -n 6 -N 6 -A 60 -a 15 -s 30 -r 20
```

`python3 scripts/parse_eclipse_output.py` collects the runs in `results/eclipse/` into
`results/eclipse/eclipse_parsed.csv`.

## Comparison with Hyperledger Fabric

The paper compares DACTIE with Hyperledger Fabric at the arrival rates of Thakkar et al.'s
Fabric benchmark (25 to 175 transactions per second): 1 KiB MISP messages for 60 s per rate,
with all archives and peers restarted between rates, measured at a peer on another host. The
comparison is indirect: Thakkar et al. measure Fabric, not a CTI-sharing system, and no
anonymous CTI-sharing prototype is available to measure against. Fabric also orders all
transactions, which DACTIE does not do and CTI sharing does not need.

## Scope

- 33.4 hours on four hosts is a soak test, not operational experience in an organization.
- Four peers say nothing about scale; results for larger networks come from the simulations.
- Delivery in the soak test shows no sustained shortfall, not zero loss.
