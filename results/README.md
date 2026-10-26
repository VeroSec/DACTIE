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
| Eclipse attacks (847 runs) | `eclipse_parsed.csv` |
| Latency table | `latency/` (see below) |
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

## Eclipse attacks: `eclipse_parsed.csv`

One row per run, 847 in all: network size, honest and attacker peers, the
attacker share (`attacker_ratio`), the share of honest peers that received the
victim's message (`message_ratio`), and the source folder of
`dactie-experiment-eclipse/` (`series`: `output` or `old_output`). Rebuild it
with `dactie-experiment-eclipse/parse_eclipse_output.py`.

## Latency: `latency/`

Four hosts on a switched LAN, 40 messages per second over the full signed and
MLS-encrypted path, latency from publication to delivery at a peer on another
host (mean / p95 in ms, 800 to 1,000 deliveries each). The paper's table:

| Payload | MISP | STIX |
| --- | --- | --- |
| 1 KiB | 21.9 / 33 | 22.0 / 35 |
| 4 KiB | 21.2 / 29 | 21.0 / 28 |
| 16 KiB | 22.1 / 30 | 21.6 / 30 |
| 64 KiB | 23.6 / 33 | 23.3 / 32 |
| 256 KiB | 26.3 / 36 | 26.5 / 36 |
| Complete event / feed (2.4 MB / 3.0 MB, 5 msg/s) | 89.8 / 99 | 102.2 / 113 |

Querying an archive for missed group updates took 105 ms on average. The raw
logs of this measurement are not part of the repository;
`latency/rerun-2026-10-22.csv` repeats it on the same testbed (below).

`latency/soak/peer_{1,2,3}.csv` is a 33.4-hour soak test with 16 KiB MISP events
at 40 messages per second, one row per receiver and 60-second window (sampled
about every 62 s): number of deliveries in the window (`n`), mean, median, p95
and maximum latency in ms, resident memory in MB and established connections.

## Latency re-measurement: `latency/rerun-2026-10-22.csv`

Four hosts (the testbed of `RUNBOOK.md`), the publisher `peer_4` at 40 msg/s (5 msg/s for
the complete MISP event and ATT&CK ICS feed), receivers `peer_1` and `peer_2` on other hosts
and `peer_3` on the publisher's host. Per run and receiver: mean and p95 latency over 1,000
deliveries, after dropping the previous run's tail and the first 2 s.

| Runs | Note |
| --- | --- |
| `misp-*`, `stix-*` | Nagle off, publisher without stem (`peer_1` is the receiver to compare with the table above) |
| `misp-16KiB-stem` | invalid: the publisher never used the stem (empty first epoch, since fixed) |
| `misp-16KiB-v2`, `misp-16KiB-stem-v2` | stem row invalid: the former stem rate limit of 20/s dropped half the messages (now 100/s) |
| `v3-off-*`, `v3-on-*` | Nagle off vs on (`--tcp-nodelay`), same day; `v3-off-misp-16KiB-stem` is the valid stem row |
