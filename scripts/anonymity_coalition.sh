#!/bin/bash
# Coalition grid of the paper (Figure 2, results/coalition/), with or without the Dandelion++
# stem: D in {12, 10, 8, 6} with mesh_n_high = 2D, |N_c| in {50, 100, 150, 200}, 5 runs of 90 s,
# coalitions of 1, 2, 5 and 10 peers and of 5/10/20/50 % of the peers, 5,000 sampled coalitions,
# harness defaults otherwise (40 msg/s network-wide, 50-100 connections per peer, random peer
# sampling, 10-100 ms simulated links, Monero's stem parameters). Every run is scored with
# first-spy, the uniform guess and the timing estimators.
#
#   scripts/anonymity_coalition.sh OUT_DIR [q ...]       (default q: 0, i.e. no stem)
#
# Each block writes OUT_DIR/q<q>/D<D>_N<N>/ (adversary_*.csv, experiment_*.csv, run.log) and a
# DONE marker; OUT_DIR/meta.txt logs every block. Resumable: after an abort, run it again and
# finished blocks are skipped. In the paper's data, results/coalition/D*_N* is q = 0,
# results/coalition/stem/q<q>/ the stem grid (q = 0.2, 0.1, 0.5) and results/coalition/timing/
# the timing run (q = 0 and 0.2). On a 64-vCPU AMD EPYC 7763 one q took about 3 hours.
# Summarize with scripts/coalition_figure_data.py and scripts/repeated_timing_summary.py.
set -u
[ $# -ge 1 ] || { echo "usage: $0 OUT_DIR [q ...]"; exit 1; }
REPO=$(cd "$(dirname "$0")/.." && pwd)
OUT=$(mkdir -p "$1" && cd "$1" && pwd); shift
QS=${*:-0}
cd "$REPO" || exit 1
cargo build --release --bin dactie-experiments 2>&1 | grep -E "^error|Finished" | tail -1
BIN=$REPO/target/release/dactie-experiments
echo "$(date -u +%FT%TZ) start/resume: host $(hostname), kernel $(uname -r), $(nproc) vCPUs, $(free -g | awk '/Mem:/{print $2}') GiB, commit $(git rev-parse --short HEAD)" >> "$OUT/meta.txt"
for Q in $QS; do
  for D in 12 10 8 6; do
    for N in 50 100 150 200; do
      B="$OUT/q$Q/D${D}_N$N"
      [ -f "$B/DONE" ] && continue
      rm -rf "$B"; mkdir -p "$B"
      echo "$(date -u +%FT%TZ) q=$Q D=$D N=$N start ($(nproc) vCPUs)" >> "$OUT/meta.txt"
      "$BIN" -n "$N" -N "$N" -S 50 -r 5 -t 90 \
        -D "$D" --mesh-n-high "$((D * 2))" --stem-fluff-probability "$Q" \
        --coalition-sizes 1,2,5,10 --adversary-fractions 0.05,0.1,0.2,0.5 \
        --adversary-trials 5000 --adversary-seed 2 --abort-after-low-delivery 0 \
        -o "$B/" > "$B/run.log" 2>&1
      rc=$?
      echo "$(date -u +%FT%TZ) q=$Q D=$D N=$N exit $rc, $(grep -c 'Delivery:' "$B/run.log") runs, below 100%: $(grep 'Delivery:' "$B/run.log" | grep -vc '100.0%')" >> "$OUT/meta.txt"
      [ $rc -eq 0 ] && touch "$B/DONE"
    done
  done
  echo "$(date -u +%FT%TZ) q=$Q DONE" >> "$OUT/meta.txt"
done
echo "$(date -u +%FT%TZ) COALITION-DONE" >> "$OUT/meta.txt"
