# Evaluation data

The data behind the paper's evaluation, as plain CSV.

## Single-attacker anonymity: `anonymity_parsed.csv`

313 runs with 40 to 850 peers on one channel (`C_max = 1`), GossipSub with
`mesh_n_low = 10`, `mesh_n = 12`, `mesh_n_high = 24`. Each row is one run;
`direct_publisher_percent2` is the share of peers that received the measurement
message directly from its source, i.e. how often a single attacker at a random
position observes the source as its first hop.

The paper compares it with the bound `(D + 1) / |N_c|`: with `D = mesh_n_high`
it holds in all runs, with `D = mesh_n` it is exceeded in 4.79% of them. The
`theoretical_bounds_d*` and `surpasses_th_d*` columns belong to an earlier form
of the bound and are not used.

## Coalition attack: `coalition/`

Two independent runs with identical parameters, `server/` and `laptop/`:
`D ∈ {6, 8, 10, 12}` with `mesh_n_high = 2D`, `|N_c| = 50, 100, 150, 200`,
coalition sizes from 1 to `|N_c| / 2`, five runs each with 5,000 sampled
coalitions per run.

`adversary_*.csv` holds one row per run, coalition size and estimator:
`estimator` is `random` (uniform guess), `first_spy`, `plurality` or
`observed_set`, and `success_rate` the share of sampled coalitions that named
the true source. `experiment_*.csv` records message delivery per run.

`coalition_parsed.csv` averages the first-spy and uniform-guess success of both
runs per `(N, D, c)` and adds the coalition bound `P_SML(c)` at `mesh_n_high`
(`bound_high`) and the ratio of the two. Regenerate it with

```bash
python3 scripts/coalition_figure_data.py results/coalition/coalition_parsed.csv \
    results/coalition/server results/coalition/laptop
```

In all 216 configurations the mean first-spy success stays below the bound, and
at most at 0.70 of it for coalitions of five or fewer peers.

## Eclipse attacks: `eclipse_parsed.csv`

One row per run: network size, honest and attacker peers, the attacker share
(`attacker_ratio`) and the share of honest peers that received the victim's
message (`message_ratio`).

## Latency: `latency/`

Four hosts on a switched LAN, 40 messages per second over the full signed and
MLS-encrypted path, latency from publication to delivery at a peer on another
host (mean / p95 in ms, 800 to 1,000 deliveries each):

| Payload | MISP | STIX |
| --- | --- | --- |
| 1 KiB | 21.9 / 33 | 22.0 / 35 |
| 4 KiB | 21.2 / 29 | 21.0 / 28 |
| 16 KiB | 22.1 / 30 | 21.6 / 30 |
| 64 KiB | 23.6 / 33 | 23.3 / 32 |
| 256 KiB | 26.3 / 36 | 26.5 / 36 |
| Complete event / feed (2.4 MB / 3.0 MB, 5 msg/s) | 89.8 / 99 | 102.2 / 113 |

Querying an archive for missed group updates took 105 ms on average.

`latency/soak/peer_{1,2,3}.csv` is a 33.4-hour soak test with 16 KiB MISP events
at 40 messages per second, one row per receiver and 60-second window (sampled
about every 62 s): number of deliveries in the window (`n`), mean, median, p95
and maximum latency in ms, resident memory in MB and established connections.

## Microbenchmarks

See `benchmark/README.md`.
