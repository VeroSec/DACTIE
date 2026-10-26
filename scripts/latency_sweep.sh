#!/bin/bash
# Latency sweep on the RUNBOOK.md testbed (minerva1-4, Phases 2-8 done), the rows of the
# paper's latency table. One run per payload: recreate peer_4 with the payload, wait until it
# publishes, collect BENCH_RX at peer_1..3; first with Nagle off (the default), then on.
# Usage: scripts/latency_sweep.sh [OUT_DIR]. The BENCH_RX lines land in OUT_DIR/raw/<run>.<receiver>;
# results/latency/raw-2026-10-26.csv holds one such sweep as CSV, see scripts/latency_summary.py.
D=${1:-latency-sweep}; mkdir -p "$D/raw"
S="ssh -q -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10 -o ServerAliveInterval=10"
r() { local h=$1; shift; for _ in 1 2 3 4 5; do $S "$h" "$@" && return 0; sleep 3; done; return 1; }
HP=". /opt/vero/dactie-helpers.sh"
MISP=misp/0dc6a09d-e8d7-48fd-a3f7-f554afb8d1b3
run() { # name file rate seconds tcp_nodelay
  local name=$1 file=$2 rate=$3 secs=$4 nd=$5 t0=""
  while [ -z "$t0" ]; do t0=$($S minerva4 'date -u +%Y-%m-%dT%H:%M:%SZ'); [ -z "$t0" ] && sleep 3; done
  r minerva4 "$HP; BENCH_PUBLISH=true BENCH_PAYLOAD_FILE=/usr/src/dactie/$file BENCH_RATE=$rate STEM_Q=0 TCP_NODELAY=$nd docker compose --profile node119 up -d --force-recreate peer_4 >/dev/null 2>&1"
  for _ in $(seq 1 150); do
    $S minerva4 "$HP; logs --since $t0 dactie-peer_4-1 | grep -q 'BENCH_PUBLISH enabled'" && break
    sleep 2
  done
  sleep "$secs"
  r minerva2 "$HP; logs --since $t0 dactie-peer_1-1 | grep BENCH_RX" > "$D/raw/$name.peer_1"
  r minerva3 "$HP; logs --since $t0 dactie-peer_2-1 | grep BENCH_RX" > "$D/raw/$name.peer_2"
  r minerva4 "$HP; logs --since $t0 dactie-peer_3-1 | grep BENCH_RX" > "$D/raw/$name.peer_3"
  r minerva4 "$HP; echo \"enabled: \$(logs --since $t0 dactie-peer_4-1 | grep -c 'BENCH_PUBLISH enabled'), stem sends: \$(logs --since $t0 dactie-peer_4-1 | grep -c 'Stem: sent own')\"" > "$D/raw/$name.peer_4"
  echo "$(date -u +%T) $name: peer_1 $(wc -l < "$D/raw/$name.peer_1"), peer_2 $(wc -l < "$D/raw/$name.peer_2"), peer_3 $(wc -l < "$D/raw/$name.peer_3") receipts; $(cat "$D/raw/$name.peer_4")"
}
recreate() { # tcp_nodelay
  r minerva2 "$HP; TCP_NODELAY=$1 docker compose --profile node117 up -d --force-recreate peer_1 >/dev/null 2>&1"
  r minerva3 "$HP; TCP_NODELAY=$1 docker compose --profile node118 up -d --force-recreate peer_2 >/dev/null 2>&1"
  r minerva4 "$HP; TCP_NODELAY=$1 BENCH_PUBLISH=false docker compose --profile node119 up -d --force-recreate peer_3 peer_4 >/dev/null 2>&1"
  sleep 30; echo "$(date -u +%T) peers recreated with TCP_NODELAY=$1"
}
sweep() { # prefix tcp_nodelay
  for size in 1KiB 4KiB 16KiB 64KiB 256KiB; do
    run $1-misp-$size $MISP.$size.json 40 32 $2
    run $1-stix-$size stix/ics-attack.$size.json 40 32 $2
  done
  run $1-misp-full $MISP.json 5 210 $2
  run $1-stix-full stix/ics-attack.json 5 210 $2
}
recreate true;  sweep off true
recreate false; sweep on false
recreate true
echo "SWEEP-DONE $(date -u +%T)"
