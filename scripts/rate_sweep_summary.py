"""Summary of the rate sweep (scripts/rate_sweep.sh): per arrival rate and receiver, the delivered
throughput, mean and p95 latency, loss and duplicates. Only the first receipt of each message
counts; later copies are duplicates. Throughput and latency use the steady window
[first receipt + 5 s, + 55 s).
Usage: python3 scripts/rate_sweep_summary.py results/latency/rate-sweep-2026-11-08"""
import csv, gzip, sys, statistics, datetime as dt
from collections import defaultdict
D = sys.argv[1]
runs = defaultdict(list)
with gzip.open(f'{D}/raw.csv.gz', 'rt') as f:
    for r in csv.DictReader(f):
        runs[(int(r['rate']), r['receiver'])].append(
            (dt.datetime.fromisoformat(r['received_at'][:26].rstrip('Z')), int(r['seq']), int(r['latency_ms'])))
with open(f'{D}/summary.csv', 'w') as o:
    o.write('rate,receiver,throughput,mean_ms,p95_ms,loss,receipts,duplicates\n')
    for (rate, recv), allpts in sorted(runs.items()):
        seen, pts = set(), []
        for p in allpts:
            if p[1] not in seen: seen.add(p[1]); pts.append(p)
        t0 = pts[0][0]
        win = [p for p in pts if 5 <= (p[0] - t0).total_seconds() < 55]
        lat = sorted(p[2] for p in win)
        loss = 1 - len(seen) / (max(seen) - min(seen) + 1)
        row = (rate, recv, len(win) / 50, statistics.mean(lat), lat[int(0.95 * (len(lat) - 1))], loss, len(pts), len(allpts) - len(pts))
        o.write(','.join(f'{x:.4g}' if isinstance(x, float) else str(x) for x in row) + '\n')
        print(f'{rate:4d} {recv}: {row[2]:6.1f}/s  mean {row[3]:8.1f} ms  p95 {row[4]:6d}  loss {loss:.2%}  dups {row[7]}')
