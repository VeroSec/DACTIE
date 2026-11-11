#!/bin/bash
# Single-attacker size series of the paper (results/sizes/, "Experimental bounds"): one network
# of N peers per block, N = 50 to 850 in steps of 50, 5 runs of 240 s each, harness defaults
# otherwise (D = 12 with mesh band 10/12/24, 40 msg/s network-wide, 50-100 connections per peer,
# random peer sampling, 10-100 ms simulated links).
#
#   scripts/anonymity_sizes.sh OUT_DIR [N ...]
#
# Each block writes OUT_DIR/N<N>/ (experiment_*.csv, run.log, series.csv with memory and CPU
# every 15 s) and a DONE marker; OUT_DIR/meta.txt logs every block. Resumable: after an abort,
# run it again and finished blocks are skipped. A block is killed above MEM_LIMIT_GIB (default
# 60) of resident memory. The paper's series took about 7 hours on a 64-vCPU AMD EPYC 7763 with
# 125 GiB; 850 peers peak at about 11 GiB. Summarize with scripts/size_figure_data.py.
set -u
[ $# -ge 1 ] || { echo "usage: $0 OUT_DIR [N ...]"; exit 1; }
REPO=$(cd "$(dirname "$0")/.." && pwd)
OUT=$(mkdir -p "$1" && cd "$1" && pwd); shift
SIZES=${*:-$(seq 50 50 850)}
LIMIT_KIB=$(( ${MEM_LIMIT_GIB:-60} * 1024 * 1024 ))
cd "$REPO" || exit 1
cargo build --release --bin dactie-experiments 2>&1 | grep -E "^error|Finished" | tail -1
BIN=$REPO/target/release/dactie-experiments
echo "$(date -u +%FT%TZ) start/resume: host $(hostname), kernel $(uname -r), $(nproc) vCPUs, $(free -g | awk '/Mem:/{print $2}') GiB, commit $(git rev-parse --short HEAD)" >> "$OUT/meta.txt"
for N in $SIZES; do
  B="$OUT/N$N"
  [ -f "$B/DONE" ] && continue
  rm -rf "$B"; mkdir -p "$B"
  echo "$(date -u +%FT%TZ) N=$N start ($(nproc) vCPUs)" >> "$OUT/meta.txt"
  echo "t_s,rss_mib,cpu_ticks" > "$B/series.csv"
  T0=$(date +%s)
  "$BIN" -n "$N" -N "$N" -S 50 -r 5 -t 240 --abort-after-low-delivery 0 -o "$B/" > "$B/run.log" 2>&1 &
  P=$!
  while kill -0 $P 2>/dev/null; do
    R=$(awk '/VmRSS/{print $2}' /proc/$P/status 2>/dev/null); K=$(awk '{print $14+$15}' /proc/$P/stat 2>/dev/null)
    echo "$(( $(date +%s) - T0 )),$(( ${R:-0} / 1024 )),${K:-0}" >> "$B/series.csv"
    [ "${R:-0}" -gt "$LIMIT_KIB" ] && { kill -9 $P; echo "KILLED above ${MEM_LIMIT_GIB:-60} GiB" >> "$B/run.log"; }
    sleep 15
  done
  wait $P; rc=$?
  echo "$(date -u +%FT%TZ) N=$N exit $rc, $(grep -c 'Delivery:' "$B/run.log") runs, below 100%: $(grep 'Delivery:' "$B/run.log" | grep -vc '100.0%'), peak $(cut -d, -f2 "$B/series.csv" | sort -n | tail -1) MiB" >> "$OUT/meta.txt"
  [ $rc -eq 0 ] && touch "$B/DONE"
done
echo "$(date -u +%FT%TZ) SIZES-DONE" >> "$OUT/meta.txt"
