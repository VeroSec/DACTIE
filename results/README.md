# Evaluation data

The data behind the paper's evaluation, as plain CSV. The anonymity experiments
ran on one 64-vCPU AMD EPYC 7763 server (Linux 6.8), all peers of a run in one
process, over simulated links of 10 to 100 ms delay by peer distance, so that a
detour via a third peer is never faster than the direct link. Each `meta.txt`
logs start, exit status and delivery of every cell; each `run.log` is the
harness output of one cell.

| Paper | Files |
| --- | --- |
| Experimental bounds (85 runs, 50 to 850 peers) | `anonymity_parsed.csv`, raw runs in `sizes/` |
| Coalition attack, Figure 2 (108 configurations) | `coalition/coalition_parsed.csv`, raw runs in `coalition/D*_N*/` |
| Stem phase, Figure 2 hollow markers (q = 0.2) | `coalition/stem/stem_q0.2.csv`, raw runs in `coalition/stem/q0.2/` |
| Timing estimators (extended version) | `coalition/timing/q0/`, `coalition/timing/q0.2/` |
| Repeated messages and PeerID rotation (extended version) | `repeated/` |
| Eclipse attacks (847 runs) | `eclipse_parsed.csv` |
| Latency table | `latency/rerun-2026-10-26.csv` (runs `off-*`, receiver `peer_1`), every receipt in `latency/raw-2026-10-26.csv` |
| Soak test (33.4 hours) | `latency/soak/` |
| Comparison with Hyperledger Fabric (extended version) | `fabric/rate-sweep-2026-11-11/comparison.csv`; DACTIE in `latency/rate-sweep-2026-11-*/`, Fabric in `fabric/rate-sweep-2026-11-11/` |
| Microbenchmark | `benchmark/README.md` |

## Single-attacker anonymity: `anonymity_parsed.csv`, `sizes/`

85 runs with 50 to 850 peers on one channel (`C_max = 1`), GossipSub with
`mesh_n_low = 10`, `mesh_n = 12`, `mesh_n_high = 24`, at 40 messages per second.
Each row is one run; `from_publisher` is the number of peers that received the
measurement message directly from its source, and `direct_publisher_percent2`
their share of the other peers, i.e. how often a single attacker at a random
position observes the source as its first hop.

The paper compares it with the bound `(D + 1) / |N_c|`: it holds in all runs,
with `D = mesh_n_high` and even with `D = mesh_n`, since the source reached at
most 11 peers directly. Regenerate the file with

```bash
python3 scripts/size_figure_data.py results/anonymity_parsed.csv results/sizes
```

## Coalition attack: `coalition/`

`D ∈ {6, 8, 10, 12}` with `mesh_n_high = 2D`, `|N_c| = 50, 100, 150, 200`, one
folder per `(D, |N_c|)`, coalition sizes from 1 to `|N_c| / 2`, five runs each
with 5,000 sampled coalitions per run.

`adversary_*.csv` holds one row per run, coalition size and estimator:
`estimator` is `random` (uniform guess among the peers outside the coalition),
`first_spy`, `plurality` or `observed_set`, and `success_rate` the share of
sampled coalitions that named the true source. `experiment_*.csv` records
message delivery and latency per run.

`coalition_parsed.csv` averages the first-spy and uniform-guess success over the
five runs per `(N, D, c)` and adds the coalition bound `P_SML(c)` at
`mesh_n_high` (`bound_high`) and the ratio of the two. Regenerate it with

```bash
python3 scripts/coalition_figure_data.py results/coalition/coalition_parsed.csv \
    results/coalition/D*_N*
```

In all 108 configurations the mean first-spy success stays below the bound, and
at most at 0.72 of it for coalitions of five or fewer peers. On average it is
5.26 times the colluders' share `c / |N_c|`; the median latency is 78 ms.

## Stem phase: `coalition/stem/`

The same grid with the Dandelion++ stem (Monero's parameters: two successors per
10-minute epoch, fail-safe embargo of 39 s on average), for
`q ∈ {0.1, 0.2, 0.5}`, the probability that a peer ends the stem. In all 240
runs the measurement message entered the stem (`publish_methods.txt`, one line
per run). `stem_q*.csv` have the columns of `coalition_parsed.csv`; regenerate
them with

```bash
python3 scripts/coalition_figure_data.py results/coalition/stem/stem_q0.2.csv \
    results/coalition/stem/q0.2/D*_N*
```

| q | First-spy / `c / \|N_c\|` (mean) | `\|N_c\| = 50`, `D = 12`: `c = 1` / `c = 5` | Median latency |
| --- | --- | --- | --- |
| no stem | 5.26 | 20.3% / 69.7% | 78 ms |
| 0.1 | 1.14 | 2.1% / 9.9% | 0.92 s |
| 0.2 | 1.12 | 2.5% / 11.3% | 0.63 s |
| 0.5 | 1.15 | 3.2% / 11.3% | 0.26 s |

The paper reports `q = 0.2`.

## Timing estimators: `coalition/timing/`

The coalition grid (`D ∈ {6, 8, 10, 12}`, `|N_c|` 50 to 200, five runs, 5,000
coalitions) without the stem (`q0/`) and with `q = 0.2` (`q0.2/`), scored with
the timing estimators of `dactie-experiment-anonymity/README.md` next to
first-spy (harness `exp/timing-estimator` `2a85af2`, 2026-11-03). Every message
reached all peers. The `run.log` files stayed on the evaluation server.

- Without the stem, `timing_delay` (mesh and exact link delays known) names the
  source in 16–53% of cases with two colluders and 86–99% with five, e.g. 98.4%
  at `|N_c| = 200`, `D = 12`, where first-spy reaches 23.3%.
- With the stem, `timing_delay` finds the diffuser and stays below first-spy in
  every configuration with `c ≥ 2`; `timing_delay_stem` is within −2.3 and +1.0
  percentage points of first-spy.

## Repeated messages: `repeated/`

Multi-message runs (harness `exp/multi-message` `a209ce7`, 2026-11-03):
`D = 12`, `|N_c| ∈ {50, 100, 200}`, M = 10 linked messages, the member's
messages spread over `k ∈ {1, 3, 5}` PeerIDs, coalitions of 1, 5, 10 and 20,
five runs with 5,000 coalitions each, every message delivered to all peers.
Regimes: `no_stem`; `stem_fixed` (q = 0.2, 600 s stem epoch, messages 5 s apart,
so all ten share one route); `stem_rotating` (q = 0.2, 5 s stem epoch, messages
35 s apart, so every message gets new successors). Columns: see
`dactie-experiment-anonymity/README.md`.

- With fixed routes, ten messages reveal no more than one (e.g. 10.0% for
  `c = 5` at `|N_c| = 50`).
- With new routes per message, pooling stays below `1 − (1 − c/|N_c|)^10` in
  every configuration, at 0.58 to 0.96 of it for `c ≥ 5`.
- Linkable messages over k PeerIDs within one stem epoch act like k epochs:
  10.0%, 26.4% and 35.7% for `k = 1, 3, 5` (`c = 5`, `|N_c| = 50`).

Both sections are reproduced by

```bash
python3 scripts/repeated_timing_summary.py results
```

## Eclipse attacks: `eclipse_parsed.csv`

One row per run, 847 in all: network size, honest and attacker peers, the
attacker share (`attacker_ratio`), the share of honest peers that received the
victim's message (`message_ratio`), and the source folder of
`dactie-experiment-eclipse/` (`series`: `output` or `old_output`). Rebuild it
with `dactie-experiment-eclipse/parse_eclipse_output.py`.

## Latency: `latency/`

Four hosts on a switched LAN (the testbed of `RUNBOOK.md`), 40 messages per
second over the full signed and MLS-encrypted path (5 for the complete MISP
event and ATT&CK ICS feed), latency from publication to delivery. The paper's
table is the run of 2026-10-26 with the released code, Nagle off and the
publisher without the stem, at `peer_1` on another host (mean / p95 in ms,
1,000 deliveries each):

| Payload | MISP | STIX |
| --- | --- | --- |
| 1 KiB | 19.1 / 27 | 19.2 / 27 |
| 4 KiB | 19.5 / 27 | 19.5 / 28 |
| 16 KiB | 20.4 / 28 | 20.3 / 28 |
| 64 KiB | 21.9 / 31 | 21.6 / 30 |
| 256 KiB | 25.5 / 33 | 25.3 / 33 |
| Complete event / feed (2.4 MB / 3.0 MB) | 104.4 / 118 | 117.5 / 134 |

`latency/rerun-2026-10-26.csv` has one row per run and receiver: runs `off-*`
with Nagle off, `on-*` with Nagle on (`TCP_NODELAY=false`), receivers `peer_1`
and `peer_2` on other hosts and `peer_3` on the publisher's host.
`latency/raw-2026-10-26.csv` lists every `BENCH_RX` line the receivers logged
during each run, in log order: the neighbor that delivered it (`via`, by node
name: the publisher `peer_4`, another peer or an archive), the publisher's
sequence number and the latency in ms. It starts with the tail of the previous
run. Regenerate the summary with

```bash
python3 scripts/latency_summary.py results/latency/raw-2026-10-26.csv results/latency/rerun-2026-10-26.csv
```

`scripts/latency_sweep.sh` runs the whole sweep against the testbed. An earlier
measurement on the same testbed (2026-08) reported 21.0 to 26.5 ms for 1 to
256 KiB and 89.8 / 102.2 ms for the complete event and feed; its raw logs were
not kept. Querying an archive for missed group updates took 105 ms on average
in that measurement (`evaluation_performance.md`).

`latency/soak/peer_{1,2,3}.csv` is a 33.4-hour soak test with 16 KiB MISP events
at 40 messages per second, one row per receiver and 60-second window (sampled
about every 62 s): number of deliveries in the window (`n`), mean, median, p95
and maximum latency in ms, resident memory in MB and established connections.

## Arrival-rate sweep and Hyperledger Fabric: `latency/rate-sweep-*`, `fabric/`

The same four hosts as the latency table (`RUNBOOK.md`, 4 vCPUs each). Every run uses:

- **Rates:** Thakkar et al.'s arrival rates of 25 to 175 per second, plus 110.
- **Payload:** the 1 KiB MISP record.
- **DACTIE settings:** stem and Nagle off.
- **Duration:** about 100 s per rate.
- **Measurement:** throughput, mean and p95 latency over 50 s of each run, 5 to 55 s after the first receipt.

`scripts/rate_sweep.sh` runs DACTIE. `fabric-comparison/` (see its README) runs Fabric 2.5.16 on the same hosts, placed like DACTIE: orderer where the AA runs, receiver on minerva2, load generator on minerva4. Both write `raw.csv.gz` with one row per receipt, and `summary.csv` from `scripts/rate_sweep_summary.py`. `cpu/` holds docker stats 40 s into each run.

| Directory | System | Notes |
| --- | --- | --- |
| `latency/rate-sweep-2026-11-08/` | DACTIE, released build `bec9c1c` | Receivers `peer_1`, `peer_2` (other hosts) and `peer_3` (publisher's host). From 100/s on, archives forwarded messages minutes late and peers delivered them twice. |
| `latency/rate-sweep-2026-11-11/` | DACTIE, `main` `398cc56` (with the dedup fix) | No duplicates at any rate. `peer_3` verified and forwarded every message (CPU as on the other peers) but logged no receipts, most likely because its topic subscription was not saved before the sweep recreated it. |
| `fabric/rate-sweep-2026-11-11/misp-1KiB/` | Fabric 2.5.16, 1 KiB MISP record | Also 200 to 1,000 per second. |
| `fabric/rate-sweep-2026-11-11/thakkar-20B/` | Fabric 2.5.16, 20-byte value as in Thakkar et al. | Within 4 ms of the 1 KiB record. |

At the receiver on minerva2 (`peer_1` / `peer0.org1`), throughput per second / mean / p95 latency in ms:

| Rate | DACTIE (2026-11-11) | Fabric (1 KiB) |
| --- | --- | --- |
| 25 | 25.0 / 24.1 / 29 | 25.2 / 561 / 1,018 |
| 100 | 100.0 / 15.6 / 23 | 100.2 / 197 / 332 |
| 125 | 125.0 / 18.4 / 28 | 125.4 / 166 / 274 |
| 150 | 140.3 / 1,845 / 5,264 | 150.0 / 141 / 231 |
| 175 | 141.4 / 4,762 / 12,867 (4.3% lost) | 174.6 / 128 / 204 |
| 1,000 | | 1,000 / 63 / 87 |

Up to 125 messages per second, DACTIE delivers within 16 to 24 ms on average. Fabric commits within 166 to 561 ms. Its latency is mostly waiting for a block to fill: 30 transactions or 1 s, so blocks are full from 50 per second on.

DACTIE saturates at about 141 per second, with each receiving peer verifying group signatures on one full core (93 to 95% CPU; archives about 80%). Fabric, whose peers check ECDSA signatures, committed every transaction up to 1,000 per second, with no node above 1.7 cores. Fabric gives clients no anonymity in this setup (X.509 identities).

`fabric/rate-sweep-2026-11-11/comparison.csv` puts the three series side by side:

```bash
python3 fabric-comparison/compare.py results/latency/rate-sweep-2026-11-11 \
    results/fabric/rate-sweep-2026-11-11/misp-1KiB results/fabric/rate-sweep-2026-11-11/thakkar-20B
```

## Earlier re-measurement: `latency/rerun-2026-10-22.csv`

The same testbed with a build from before the release, which also measured the stem and
Nagle on vs off. The publisher `peer_4` at 40 msg/s (5 msg/s for
the complete MISP event and ATT&CK ICS feed), receivers `peer_1` and `peer_2` on other hosts
and `peer_3` on the publisher's host. Per run and receiver: mean and p95 latency over 1,000
deliveries, after dropping the previous run's tail and the first 2 s.

| Runs | Note |
| --- | --- |
| `misp-*`, `stix-*` | Nagle off, publisher without stem (`peer_1` is the receiver to compare with the table above) |
| `misp-16KiB-stem` | invalid: the publisher never used the stem (empty first epoch, since fixed) |
| `misp-16KiB-v2`, `misp-16KiB-stem-v2` | stem row invalid: the former stem rate limit of 20/s dropped half the messages (now 100/s) |
| `v3-off-*`, `v3-on-*` | Nagle off vs on (`--tcp-nodelay`), same day; `v3-off-misp-16KiB-stem` is the valid stem row |
