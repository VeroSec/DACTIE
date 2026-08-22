#!/bin/bash
# Sample receive latency once a minute during a soak and append a CSV row.
#
#   ./scripts/soak_sampler.sh <compose-profile> <service> <out.csv>
#
# Reads only the last window of container logs, so it stays cheap and is immune
# to log rotation: the CSV, not the docker log, is the durable time series.
set -uo pipefail
PROFILE="${1:?profile}"; SERVICE="${2:?service}"; OUT="${3:?out.csv}"
WINDOW="${WINDOW:-60}"

[ -f "$OUT" ] || echo "ts,service,n,mean_ms,p50_ms,p95_ms,max_ms,rss_mb,established_conns" > "$OUT"

while true; do
    TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

    STATS=$(sudo docker compose --profile "$PROFILE" logs --since "${WINDOW}s" "$SERVICE" 2>/dev/null \
        | sed 's/\x1b\[[0-9;]*m//g' \
        | grep -aoE 'latency_ms=[0-9]+' | cut -d= -f2 | sort -n \
        | awk '{a[NR]=$1; s+=$1}
               END { if (NR==0) { print "0,,,,"; exit }
                     printf "%d,%.1f,%d,%d,%d\n", NR, s/NR, a[int(NR*0.5)], a[int(NR*0.95)], a[NR] }')

    CID=$(sudo docker compose --profile "$PROFILE" ps -q "$SERVICE" 2>/dev/null | head -1)
    RSS=""
    [ -n "$CID" ] && RSS=$(sudo docker stats --no-stream --format '{{.MemUsage}}' "$CID" 2>/dev/null \
        | awk '{print $1}' | sed 's/MiB//;s/GiB/*1024/' | bc -l 2>/dev/null | cut -d. -f1)
    CONNS=$(ss -tn state established 2>/dev/null | grep -c ':21' || echo 0)

    echo "$TS,$SERVICE,$STATS,$RSS,$CONNS" >> "$OUT"
    sleep "$WINDOW"
done
