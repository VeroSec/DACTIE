#!/bin/bash
# Arrival-rate sweep on the docs/RUNBOOK.md testbed (minerva1-4, Phases 2-8 done), for the comparison
# with Thakkar et al.'s Fabric benchmark: 1 KiB MISP, stem off, Nagle off. The publisher runs 60 s
# after its start line plus the docker stats round and its own stop, about 100 s per rate in the
# 2026-11-08 sweep; the summary uses 50 s of each run (5 to 55 s after the first receipt).
# Before each rate, both archives and all four peers are recreated (state kept), so that the
# backlog of a saturated run does not leak into the next. Mid-run (t+40 s), docker stats on every
# host shows which node saturates.
# Usage: [RATES="25 50"] [DACTIE_DIR=dactie] scripts/rate_sweep.sh [OUT_DIR]. DACTIE_DIR is the
# checkout on the hosts that the network runs from, relative to the home directory there; its
# directory name is the compose project and so the
# prefix of the container names, which lets a sweep run from a second checkout beside another network.
# BENCH_RX lines land in OUT_DIR/r<rate>.<receiver>;
# scripts/rate_sweep_raw_csv.py turns them into OUT_DIR/raw.csv.gz (results/latency/rate-sweep-2026-11-08/
# holds the paper's sweep), and scripts/rate_sweep_summary.py summarizes it.
D=${1:-rate-sweep}; mkdir -p "$D"
S="ssh -q -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10 -o ServerAliveInterval=10"
r() { local h=$1; shift; for _ in 1 2 3 4 5; do $S "$h" "$@" && return 0; sleep 3; done; return 1; }
DIR=${DACTIE_DIR:-dactie}
P=$(basename "$DIR")
HP="cd $DIR; logs() { docker logs \"\$@\" 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; }"
FILE=/usr/src/dactie/workloads/misp/0dc6a09d-e8d7-48fd-a3f7-f554afb8d1b3.1KiB.json
for rate in ${RATES:-25 50 75 100 110 125 150 175}; do
  r minerva2 "$HP; docker compose --profile node117 up -d --force-recreate archive_node1 peer_1 >/dev/null 2>&1" &
  r minerva3 "$HP; docker compose --profile node118 up -d --force-recreate archive_node2 peer_2 >/dev/null 2>&1" &
  r minerva4 "$HP; BENCH_PUBLISH=false docker compose --profile node119 up -d --force-recreate peer_3 peer_4 >/dev/null 2>&1" &
  wait; sleep 40   # meshes re-form
  t0=$($S minerva4 'date -u +%Y-%m-%dT%H:%M:%SZ')
  r minerva4 "$HP; BENCH_PUBLISH=true BENCH_PAYLOAD_FILE=$FILE BENCH_RATE=$rate STEM_Q=0 TCP_NODELAY=true docker compose --profile node119 up -d --force-recreate peer_4 >/dev/null 2>&1"
  for _ in $(seq 1 150); do $S minerva4 "$HP; logs --since $t0 $P-peer_4-1 | grep -q 'BENCH_PUBLISH enabled'" && break; sleep 2; done
  sleep 40
  for h in minerva1 minerva2 minerva3 minerva4; do echo "== $h"; $S $h 'docker stats --no-stream --format "{{.Name}} {{.CPUPerc}}" | grep dactie'; done > "$D/r$rate.cpu"
  sleep 20
  r minerva4 "$HP; BENCH_PUBLISH=false docker compose --profile node119 up -d --force-recreate peer_4 >/dev/null 2>&1"
  sleep 20
  r minerva2 "$HP; logs -t --since $t0 $P-peer_1-1 | grep BENCH_RX" > "$D/r$rate.peer_1"
  r minerva3 "$HP; logs -t --since $t0 $P-peer_2-1 | grep BENCH_RX" > "$D/r$rate.peer_2"
  r minerva4 "$HP; logs -t --since $t0 $P-peer_3-1 | grep BENCH_RX" > "$D/r$rate.peer_3"
  echo "$(date -u +%T) rate $rate: $(wc -l < $D/r$rate.peer_1) / $(wc -l < $D/r$rate.peer_2) / $(wc -l < $D/r$rate.peer_3) receipts; cpu: $(tr '\n' ' ' < $D/r$rate.cpu)"
done
echo SWEEP-DONE
