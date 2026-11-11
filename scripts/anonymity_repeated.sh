#!/bin/bash
# Repeated messages and PeerID rotation (results/repeated/): D = 12 with mesh_n_high 24,
# |N_c| in {50, 100, 200}, M = 10 linked messages per source member spread over k in {1, 3, 5}
# PeerIDs (one run serves all k), coalitions of 1, 5, 10 and 20 peers, 5 runs of 90 s,
# 5,000 sampled coalitions, harness defaults otherwise. Three regimes:
#   no_stem        messages 5 s apart
#   stem_fixed     q = 0.2, 600 s stem epoch, messages 5 s apart: all ten share one route
#   stem_rotating  q = 0.2, 5 s stem epoch, messages 35 s apart (>= epoch + 30 s jitter):
#                  new successors for every message
#
#   scripts/anonymity_repeated.sh OUT_DIR
#
# Each block writes OUT_DIR/<regime>/N<N>/ (multi_message_*.csv, run.log) and a DONE marker;
# OUT_DIR/meta.txt logs every block. Resumable: after an abort, run it again and finished blocks
# are skipped. On a 64-vCPU AMD EPYC 7763 the three regimes took about 4 hours.
# Summarize with scripts/repeated_timing_summary.py.
set -u
[ $# -eq 1 ] || { echo "usage: $0 OUT_DIR"; exit 1; }
REPO=$(cd "$(dirname "$0")/.." && pwd)
OUT=$(mkdir -p "$1" && cd "$1" && pwd)
cd "$REPO" || exit 1
cargo build --release --bin dactie-experiments 2>&1 | grep -E "^error|Finished" | tail -1
BIN=$REPO/target/release/dactie-experiments
log() { echo "$(date -u +%FT%TZ) $*" >> "$OUT/meta.txt"; }
log "start/resume: host $(hostname), kernel $(uname -r), $(nproc) vCPUs, $(free -g | awk '/Mem:/{print $2}') GiB, commit $(git rev-parse --short HEAD)"
for R in no_stem stem_fixed stem_rotating; do
  case $R in
    no_stem)       F="--message-spacing-secs 5" ;;
    stem_fixed)    F="--stem-fluff-probability 0.2 --stem-epoch-secs 600 --message-spacing-secs 5" ;;
    stem_rotating) F="--stem-fluff-probability 0.2 --stem-epoch-secs 5 --message-spacing-secs 35" ;;
  esac
  for N in 50 100 200; do
    B="$OUT/$R/N$N"
    [ -f "$B/DONE" ] && continue
    rm -rf "$B"; mkdir -p "$B"
    log "$R N=$N start ($(nproc) vCPUs) flags: $F"
    # shellcheck disable=SC2086
    "$BIN" -n "$N" -N "$N" -S 50 -r 5 -t 90 -D 12 --mesh-n-high 24 \
      --messages-per-source 10 --peer-ids-per-source 1,3,5 \
      --coalition-sizes 1,5,10,20 --adversary-fractions 0.1 \
      --adversary-trials 5000 --adversary-seed 2 $F \
      -o "$B/" > "$B/run.log" 2>&1
    rc=$?
    log "$R N=$N exit $rc, $(grep -c 'Delivery:' "$B/run.log") runs, below 100%: $(grep 'Delivery:' "$B/run.log" | grep -vc '100.0%')"
    [ $rc -eq 0 ] && touch "$B/DONE"
  done
done
log "REPEATED-DONE"
