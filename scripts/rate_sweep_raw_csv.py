#!/usr/bin/env python3
"""Turn a rate sweep's raw BENCH_RX logs into the raw.csv.gz that scripts/rate_sweep_summary.py reads.

    python3 scripts/rate_sweep_raw_csv.py OUT_DIR

scripts/rate_sweep.sh writes OUT_DIR/r<rate>.<receiver>: the BENCH_RX lines one receiver logged
during one rate, each prefixed with docker's receive timestamp (`docker logs -t`). This writes
OUT_DIR/raw.csv.gz with one row per line (rate, receiver, received_at, via, seq, latency_ms), the
format of results/latency/rate-sweep-2026-11-08/raw.csv.gz; `via` is the PeerID of the neighbour
that delivered the message. The OUT_DIR/r<rate>.cpu files are skipped.
"""
import csv
import gzip
import os
import re
import sys

NAME = re.compile(r"^r(\d+)\.(peer_\d+)$")
LINE = re.compile(r"^(\S+) .*?BENCH_RX .*?via=(\S+) seq=(\d+) latency_ms=(\d+)")


def main(out_dir):
    files = sorted(f for f in os.listdir(out_dir) if NAME.match(f))
    n = 0
    with gzip.open(os.path.join(out_dir, "raw.csv.gz"), "wt", newline="") as fh:
        w = csv.writer(fh, lineterminator="\n")
        w.writerow(["rate", "receiver", "received_at", "via", "seq", "latency_ms"])
        for f in files:
            rate, receiver = NAME.match(f).groups()
            for line in open(os.path.join(out_dir, f), errors="replace"):
                m = LINE.search(line)
                if m:
                    received_at, via, seq, latency = m.groups()
                    w.writerow([rate, receiver, received_at, via, seq, latency])
                    n += 1
    print(f"{n} receipts from {len(files)} files written to {out_dir}/raw.csv.gz")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
