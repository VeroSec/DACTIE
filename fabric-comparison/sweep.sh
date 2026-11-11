#!/bin/bash
# Fabric arrival-rate sweep, done like scripts/rate_sweep.sh does it for DACTIE. Before each
# rate, every Fabric container is recreated (ledgers kept), so that the backlog of a saturated
# run does not leak into the next. The load generator runs on the host of peer1.org1 (where
# DACTIE's benchmark publisher runs), uses peer1.org1 as its gateway, submits at the rate for
# DURATION seconds and timestamps each transaction when its block reaches peer0.org1 (the host
# of DACTIE's receiver peer_1). Mid-run (t+40 s), docker stats on every host.
# Usage: [RATES="25 50"] [DURATION=100] ./sweep.sh misp|thakkar OUT_DIR
#   misp:    the DACTIE sweep's 1 KiB MISP record in every transaction
#   thakkar: Thakkar et al.'s workload, a 20-byte value
# OUT_DIR gets r<rate>.receipts.csv, r<rate>.failures.csv, r<rate>.log, cpu/r<rate>.cpu and
# raw.csv.gz (all receipts), then the summary of scripts/rate_sweep_summary.py.
set -euo pipefail
source "$(dirname "$0")/common.sh"
WORKLOAD=${1:?usage: sweep.sh misp|thakkar OUT_DIR}
OUT=$(realpath -m "${2:?usage: sweep.sh misp|thakkar OUT_DIR}")
case $WORKLOAD in
    misp) PAYLOAD="-payload-file /work/misp-1KiB.json" ;;
    thakkar) PAYLOAD="-payload-bytes 20" ;;
    *) echo "unknown workload $WORKLOAD" >&2; exit 2 ;;
esac
cd "$HERE"
load_env build/hosts.env
BASE=$(base_dir)
DUSER=$(host_docker_user)
RUN_DIR=$BASE/out/$(basename "$OUT") # where the load generator writes, on its host
[ -z "$P1ORG1_SSH" ] && RUN_DIR=$OUT
COMPOSE="docker compose -f network/compose.yaml --env-file build/hosts.env"
O1=/work/crypto/peerOrganizations/org1.example.com
mkdir -p "$OUT/cpu"
on "$P1ORG1_SSH" "mkdir -p $RUN_DIR"

for rate in ${RATES:-25 50 75 100 110 125 150 175}; do
    while IFS='|' read -r host profiles; do
        on "$host" "cd $BASE && $COMPOSE $profiles up -d --force-recreate >/dev/null 2>&1" &
    done < <(host_profiles)
    wait
    sleep 20 # Raft leader, peers back on the channel

    on "$P1ORG1_SSH" "docker run --rm --network host $DUSER -v $BASE/build:/work:ro -v $RUN_DIR:/out \
        dactie-fabric-loadgen:1 -rate $rate -duration ${DURATION:-100}s $PAYLOAD \
        -gateway $P1ORG1_IP:9051 -gateway-tls-ca $O1/tlsca/tlsca.org1.example.com-cert.pem \
        -events $P0ORG1_IP:7051 -events-tls-ca $O1/tlsca/tlsca.org1.example.com-cert.pem -receiver peer0.org1 \
        -msp Org1MSP -cert $O1/users/User1@org1.example.com/msp/signcerts/User1@org1.example.com-cert.pem \
        -key $O1/users/User1@org1.example.com/msp/keystore/priv_sk \
        -out /out/r$rate.receipts.csv -failures /out/r$rate.failures.csv" > "$OUT/r$rate.log" 2>&1 &
    loadgen=$!
    sleep 42 # 2 s until the load generator submits, then 40 s into the run
    while IFS='|' read -r host profiles; do
        echo "== ${host:-local}"
        on "$host" "docker stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}' | grep fabric"
    done < <(host_profiles) > "$OUT/cpu/r$rate.cpu"
    wait $loadgen || echo "rate $rate: load generator exited with $?" >&2
    if [ -n "$P1ORG1_SSH" ]; then
        scp -q "$P1ORG1_SSH:$RUN_DIR/r$rate.*.csv" "$OUT/"
    fi
    echo "$(date -u +%T) rate $rate: $(tail -1 "$OUT/r$rate.log")"
done

python3 - "$OUT" <<'EOF'
import csv, glob, gzip, sys
out = sys.argv[1]
files = sorted(glob.glob(f'{out}/r*.receipts.csv'), key=lambda p: int(p.rsplit('/r', 1)[1].split('.')[0]))
with gzip.open(f'{out}/raw.csv.gz', 'wt', newline='') as raw:
    w = csv.writer(raw)
    w.writerow(['rate', 'receiver', 'received_at', 'seq', 'latency_ms', 'block'])
    for path in files:
        with open(path) as f:
            r = csv.reader(f)
            next(r)
            w.writerows(r)
EOF
python3 ../scripts/rate_sweep_summary.py "$OUT"
echo "SWEEP-DONE"
