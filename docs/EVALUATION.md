# Evaluation

How every result of the paper was produced, how to run it again, and where its data is. The
data files and their columns are described in [results/README.md](../results/README.md).

## Overview

| Result | Where it runs | Run | Summarize | Data |
| --- | --- | --- | --- | --- |
| End-to-end latency, MISP and STIX | testbed, four hosts | `scripts/latency_sweep.sh` | `scripts/latency_raw_csv.py`, `scripts/latency_summary.py` | `results/latency/` |
| Comparison with Hyperledger Fabric | testbed | `scripts/rate_sweep.sh`, `experiments/fabric/sweep.sh` | `scripts/rate_sweep_raw_csv.py`, `scripts/rate_sweep_summary.py`, `experiments/fabric/compare.py` | `results/latency/rate-sweep-2026-11-*/`, `results/fabric/` |
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

Most related work builds on Hyperledger Fabric. Thakkar et al. benchmarked it at arrival rates
of 25 to 175 transactions per second, with Fabric 1.0 on 32-vCPU nodes. We ran their rates with
both systems on the testbed:

- **Software:** DACTIE with `scripts/rate_sweep.sh`; Fabric 2.5.16 with `experiments/fabric/`
  (setup in its README).
- **Placement:** the same for both. Fabric's orderer runs where DACTIE's AA runs, the receiver
  is on minerva2, and the publisher or load generator is on minerva4.
- **Workload:** every message or transaction carries the same 1 KiB MISP record. Each rate
  runs about 100 s; the DACTIE publisher stops 60 s after its start line plus the
  `docker stats` round and its own stop.
- **Between rates:** all nodes are recreated, so that the backlog of a saturated rate does not
  leak into the next.
- **Measurement:** throughput and latency use the 50 s after the first 5 s of each run; only
  the first receipt of a message counts.
- **Settings:** DACTIE runs without the stem and with Nagle off. Fabric gets its best case:
  one Raft orderer, one endorsing peer, LevelDB, and blocks of 30 transactions or 1 s, as in
  Thakkar et al.

Both systems were measured on 2026-11-11:

| Arrival rate (per s) | DACTIE at `peer_1`: throughput, mean / p95 | Fabric at `peer0.org1`: throughput, mean / p95 |
| --- | --- | --- |
| 25 | 25.0, 24.1 / 29 ms | 25.2, 561 / 1,018 ms |
| 50 | 50.0, 17.3 / 26 ms | 49.8, 345 / 615 ms |
| 75 | 75.0, 15.7 / 23 ms | 75.0, 247 / 426 ms |
| 100 | 100.0, 15.6 / 23 ms | 100.2, 197 / 332 ms |
| 110 | 110.0, 15.8 / 24 ms | 109.8, 182 / 305 ms |
| 125 | 125.0, 18.4 / 28 ms | 125.4, 166 / 274 ms |
| 150 | 140.3, 1,845 / 5,264 ms | 150.0, 141 / 231 ms |
| 175 | 141.4, 4,762 / 12,867 ms, 4.3 % lost | 174.6, 128 / 204 ms |
| 1,000 | – | 1,000, 63 / 87 ms |

**Latency.** Up to 125 messages per second, DACTIE delivers within 16 to 24 ms on average and
Fabric commits within 166 to 561 ms. Most of Fabric's latency is waiting for a block to fill:
30 transactions or 1 s, and from 50 per second on every block is full.

**Saturation.** DACTIE saturates at about 141 messages per second, with one CPU core busy at
each receiving peer (93–95 % in `results/latency/rate-sweep-2026-11-11/cpu/r150.cpu`). That is
close to the rate that verifying the group signatures one after another allows; verifying them
in parallel would raise it. Fabric committed every transaction at every rate up to 1,000 per
second, with no node above 1.7 cores. Its peers check ECDSA signatures, and its clients are not
anonymous in this setup; with anonymous clients, see below.

**Thakkar et al.'s workload.** Their 20-byte value
(`results/fabric/rate-sweep-2026-11-11/thakkar-20B/`) changes Fabric's numbers by less than
4 ms. Their Fabric 1.0 took 421 to 820 ms and saturated at about 140 per second.

**Anonymous Fabric clients (Idemix).** In the runs above, Fabric's client signs with an X.509
certificate, so every transaction names its submitter. Fabric's counterpart of DACTIE's
anonymous senders is its Idemix MSP.
- **Setup:** on 2026-11-13 we added an Idemix organisation to the channel. Every transaction
  carries a fresh pseudonym with a fresh proof of membership (`CLIENT=idemix`, setup in
  `experiments/fabric/README.md`), so transactions are as unlinkable as DACTIE's messages.
  Fabric's own Idemix client reuses one pseudonym, which makes its transactions linkable.
- **Baseline:** the X.509 client ran again on the same deployment in the same session, and
  reproduced the 2026-11-11 numbers within 3 ms.

| Arrival rate (per s) | Fabric, X.509 client (rerun): throughput, mean / p95 | Fabric, Idemix client: throughput, mean / p95 |
| --- | --- | --- |
| 5 | – | 5.0, 937 / 1,445 ms |
| 10 | – | 10.0, 942 / 1,421 ms |
| 15 | – | 15.0, 920 / 1,395 ms |
| 20 | – | 20.1, 2,515 / 3,764 ms |
| 25 | 25.0, 564 / 1,016 ms | 21.1, 7,438 / 14,639 ms |
| 50 | – | 13.6, 23.2 / 39.8 s, 57 % lost |
| 75 | – | 7.3, 26.2 / 47.0 s, 79 % lost |
| 100 | 100.2, 199 / 334 ms | 3.5, 35.6 / 44.8 s, 84 % lost |
| 110 | – | 20.8, 22.4 / 33.9 s, 77 % lost |
| 125 | – | 3.6, 22.4 / 37.5 s, 88 % lost |
| 150 | – | 20.8, 17.2 / 29.6 s, 80 % lost |
| 175 | 174.6, 128 / 205 ms | 3.5, 41.5 / 59.8 s, 94 % lost |
| 500 | 500.4, 72 / 99 ms | – |

- **Cost per transaction**, on one core of minerva4
  (`results/fabric/rate-sweep-2026-11-13-idemix/idemix-cost-minerva4.txt`):
  - the client needs 19.1 ms for a fresh pseudonym with its proof, derived before the run, and
    2.2 ms per signature, two per transaction during the run;
  - every node that checks the transaction needs 35.0 ms for the proof (a pairing check) and
    3.6 ms for the signature;
  - a DACTIE receiver spends about 7 ms per message.
- **Saturation:**
  - Fabric with anonymous clients keeps up to about 20 transactions per second.
  - Up to 15 per second, every transaction commits within 0.92 to 0.94 s on average, against
    564 ms with the X.509 client at 25 per second. Below 30 per second, both wait for the 1 s
    block timeout.
  - At 20 per second, latency rises to 2.5 s. At 25 per second, Fabric commits 21 per second
    after 7.4 s on average.
  - From 50 per second on, the gateways reject most transactions ("too many requests"; a peer
    takes 500 requests at once by default). 57 to 94 % are lost, and the rest take 17 to 42 s.
- **Why throughput is so low:**
  - Every proof is checked by the endorsing peer, the orderer and all four committing peers.
  - Fabric 2.5 checks it again for every principal it tests while evaluating the `Writers`
    policies. The Idemix MSP's `SatisfiesPrincipal` validates the identity first, its cache
    is keyed by identity and principal, and `ImplicitMeta` policies try their organisations
    in random order.
  - From CPU use at 5 per second, the orderer checked each proof about four times and the
    gateway peer about six times, a committing peer once.
  - Above saturation, the limiting node varies between rates, because the nodes restart for
    every rate and draw a new order. The orderer limited at 25, 50, 75, 110 and 150 per second,
    the endorsing peers at 100, 125 and 175 (`cpu/`).
  - Even with one check per node, 38.6 ms per transaction caps every node with four vCPUs at
    about 100 transactions per second.

Anonymity thus costs Fabric far more than it costs DACTIE. On the same hosts and with the same
record, Fabric with anonymous clients keeps up to about 20 transactions per second, against
DACTIE's 141 messages per second. Below saturation, its latency is 0.9 s, against DACTIE's 16
to 24 ms.

**The sweep of 2026-11-08.** It ran the released build `bec9c1c` and gave the same DACTIE
numbers within a few milliseconds, with 13.7 % lost at 175 per second. From 100 messages per
second on, the archives fell behind and forwarded some messages again after GossipSub's 60 s
duplicate cache had expired, so peers received them twice (the `duplicates` column). Peers now
drop such late copies (`crates/dactie-utils/src/dedup.rs`), and the 2026-11-11 rerun has none.

Fabric's latency also includes ordering all transactions, which DACTIE does not do and CTI
sharing does not need.

To run both from a machine with ssh access to the four hosts (about 4 minutes per rate for
DACTIE, about 2.5 minutes for Fabric):

```bash
scripts/rate_sweep.sh OUT_DIR                      # raw BENCH_RX lines in OUT_DIR/r<rate>.<receiver>
python3 scripts/rate_sweep_raw_csv.py OUT_DIR      # OUT_DIR/raw.csv.gz
python3 scripts/rate_sweep_summary.py OUT_DIR      # OUT_DIR/summary.csv
cd experiments/fabric
./generate.sh hosts/minervas.env && ./deploy.sh    # Fabric network on the four hosts
./sweep.sh misp FABRIC_OUT_DIR                     # FABRIC_OUT_DIR/raw.csv.gz and summary.csv
python3 compare.py OUT_DIR FABRIC_OUT_DIR > comparison.csv
# anonymous clients: a channel with the Idemix organisation, in /opt/vero/fabric-idemix
IDEMIX=1 ./generate.sh hosts/minervas-idemix.env && ./deploy.sh
RATES="5 10 15 20 25 50 75 100 110 125 150 175" CLIENT=idemix ./sweep.sh misp IDEMIX_OUT_DIR
python3 compare.py OUT_DIR FABRIC_OUT_DIR fabricidemix=IDEMIX_OUT_DIR > comparison.csv
```

`scripts/rate_sweep_raw_csv.py` and `scripts/rate_sweep_summary.py` regenerate
`results/latency/rate-sweep-2026-11-08/summary.csv` byte for byte from the sweep's raw lines.
Stop the DACTIE network before running Fabric, and the other way round: they would share the
hosts' CPUs. During the Idemix sweep, the DACTIE demo network stayed up but idle (load average
0.00). The X.509 rerun in the same session matched the 2026-11-11 sweep, for which it was
stopped.

## Scope

- 33.4 hours on four hosts is a soak test, not operational experience in an organization.
- Four peers say nothing about scale; results for larger networks come from the simulations.
- Delivery in the soak test shows no sustained shortfall, not zero loss.
