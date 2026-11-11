#!/bin/bash
# Arrival-rate sweep on the RUNBOOK.md testbed (minerva1-4, Phases 2-8 done), for the comparison
# with Hyperledger Fabric (fabric-comparison/): 1 KiB MISP, stem off, Nagle off. The publisher runs 60 s
# after its start line plus the docker stats round and its own stop, about 100 s per rate in the
# 2026-11-08 sweep; the summary uses 50 s of each run (5 to 55 s after the first receipt).
# Before each rate, both archives and all four peers are recreated (state kept), so that the
# backlog of a saturated run does not leak into the next. Mid-run (t+40 s), docker stats on every
# host shows which node saturates.
# Usage: [RATES="25 50"] [DACTIE_DIR=/opt/vero/dactie] scripts/rate_sweep.sh [OUT_DIR]
#   DACTIE_DIR is the checkout on the hosts the network runs from; its compose project (and so the
#   container names) is the directory name, as docker compose derives it.
# BENCH_RX lines land in OUT_DIR/r<rate>.<receiver>, docker stats in OUT_DIR/cpu/; at the end all
# receipts go to OUT_DIR/raw.csv.gz and scripts/rate_sweep_summary.py summarizes them.
# The payload is copied from the checkout's misp/ into peer_4's key material directory, which the
# compose file mounts: images built since the Docker cleanup (650de29) contain no misp/ directory.
D=${1:-rate-sweep}; mkdir -p "$D/cpu"
DIR=${DACTIE_DIR:-/opt/vero/dactie}
P=$(basename "$DIR")
S="ssh -q -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10 -o ServerAliveInterval=10"
r() { local h=$1; shift; for _ in 1 2 3 4 5; do $S "$h" "$@" && return 0; sleep 3; done; return 1; }
HP="cd $DIR; logs() { docker logs \"\$@\" 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; }"
MISP=0dc6a09d-e8d7-48fd-a3f7-f554afb8d1b3.1KiB.json
r minerva4 "$HP; docker run --rm -v \$PWD/key_material/key_material_6:/km -v \$PWD/misp:/misp:ro busybox cp /misp/$MISP /km/bench-payload.json" ||
  { echo "could not place the payload on minerva4" >&2; exit 1; }
FILE=/usr/src/dactie/key_material/bench-payload.json
for rate in ${RATES:-25 50 75 100 110 125 150 175}; do
  r minerva2 "$HP; docker compose --profile node117 up -d --force-recreate archive_node1 peer_1 >/dev/null 2>&1" &
  r minerva3 "$HP; docker compose --profile node118 up -d --force-recreate archive_node2 peer_2 >/dev/null 2>&1" &
  r minerva4 "$HP; BENCH_PUBLISH=false docker compose --profile node119 up -d --force-recreate peer_3 peer_4 >/dev/null 2>&1" &
  wait; sleep 40   # meshes re-form
  t0=$($S minerva4 'date -u +%Y-%m-%dT%H:%M:%SZ')
  r minerva4 "$HP; BENCH_PUBLISH=true BENCH_PAYLOAD_FILE=$FILE BENCH_RATE=$rate STEM_Q=0 TCP_NODELAY=true docker compose --profile node119 up -d --force-recreate peer_4 >/dev/null 2>&1"
  for _ in $(seq 1 150); do $S minerva4 "$HP; logs --since $t0 $P-peer_4-1 | grep -q 'BENCH_PUBLISH enabled'" && break; sleep 2; done
  [ -s "$D/payload.log" ] || r minerva4 "$HP; logs --since $t0 $P-peer_4-1 | grep -m1 'BENCH_PAYLOAD_FILE'" > "$D/payload.log"
  sleep 40
  for h in minerva1 minerva2 minerva3 minerva4; do echo "== $h"; $S $h 'docker stats --no-stream --format "{{.Name}} {{.CPUPerc}}" | grep dactie'; done > "$D/cpu/r$rate.cpu"
  sleep 20
  r minerva4 "$HP; BENCH_PUBLISH=false docker compose --profile node119 up -d --force-recreate peer_4 >/dev/null 2>&1"
  sleep 20
  r minerva2 "$HP; logs -t --since $t0 $P-peer_1-1 | grep BENCH_RX" > "$D/r$rate.peer_1"
  r minerva3 "$HP; logs -t --since $t0 $P-peer_2-1 | grep BENCH_RX" > "$D/r$rate.peer_2"
  r minerva4 "$HP; logs -t --since $t0 $P-peer_3-1 | grep BENCH_RX" > "$D/r$rate.peer_3"
  echo "$(date -u +%T) rate $rate: $(wc -l < $D/r$rate.peer_1) / $(wc -l < $D/r$rate.peer_2) / $(wc -l < $D/r$rate.peer_3) receipts; cpu: $(tr '\n' ' ' < $D/cpu/r$rate.cpu)"
done
python3 - "$D" <<'EOF'
import csv, glob, gzip, re, sys
d = sys.argv[1]
rx = re.compile(r'^(\S+)\s.*BENCH_RX\s+topic=\S+\s+via=(\S+)\s+seq=(\d+)\s+latency_ms=(\d+)')
files = sorted(glob.glob(f'{d}/r*.peer_*'))  # file name order, as in the 2026-11-08 raw.csv.gz
with gzip.open(f'{d}/raw.csv.gz', 'wt', newline='') as out:
    w = csv.writer(out, lineterminator='\n')
    w.writerow(['rate', 'receiver', 'received_at', 'via', 'seq', 'latency_ms'])
    for f in files:
        rate, recv = re.search(r'/r(\d+)\.(peer_\d+)$', f).groups()
        for line in open(f):
            if m := rx.search(line):
                w.writerow([rate, recv, m.group(1), m.group(2), m.group(3), m.group(4)])
EOF
python3 "$(dirname "$0")/rate_sweep_summary.py" "$D"
echo SWEEP-DONE
