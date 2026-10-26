#!/usr/bin/env python3
"""Mean and p95 latency per run and receiver from a latency sweep's raw receipts.

    python3 scripts/latency_summary.py results/latency/raw-2026-10-26.csv OUT.csv

The raw file lists every BENCH_RX line a receiver logged since its run started, in log
order, so it begins with the tail of the previous run. Per run and receiver this keeps
the receipts after the publisher's sequence number restarted, drops the first 2 s
(seq < 2 * rate; rate 5 msg/s for the complete event and feed, else 40), and takes the
next 1,000 deliveries.
"""
import collections
import csv
import statistics as st
import sys


def main(raw, out):
    cells = collections.OrderedDict()
    for r in csv.DictReader(open(raw)):
        cells.setdefault((r["run"], r["receiver"]), []).append((int(r["seq"]), int(r["latency_ms"])))
    rows = []
    for (run, receiver), rx in cells.items():
        rate = 5 if "full" in run else 40
        start = next((i for i in range(1, len(rx)) if rx[i][0] < rx[i - 1][0]), 0)
        lat = [l for s, l in rx[start:] if s >= 2 * rate][:1000]
        if not lat:
            continue
        ls = sorted(lat)
        rows.append(dict(run=run, receiver=receiver, mean_ms=round(st.mean(lat), 1),
                         p95_ms=ls[max(0, int(round(0.95 * len(ls))) - 1)], n=len(lat)))
    with open(out, "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["run", "receiver", "mean_ms", "p95_ms", "n"])
        w.writeheader()
        w.writerows(rows)
    print(f"{len(rows)} rows written to {out}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
