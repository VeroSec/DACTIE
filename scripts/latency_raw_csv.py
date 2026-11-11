#!/usr/bin/env python3
"""Turn a latency sweep's raw BENCH_RX logs into the CSV that scripts/latency_summary.py reads.

    python3 scripts/latency_raw_csv.py OUT_DIR/raw OUT.csv [PEERS]

scripts/latency_sweep.sh writes OUT_DIR/raw/<run>.<receiver>: the BENCH_RX lines one receiver
logged during one run, in log order. This writes one row per line (run, receiver, via, seq,
latency_ms), the format of results/latency/raw-2026-10-26.csv, taking the files in the order the
sweep wrote them. `via` is the PeerID of the neighbour that delivered the message; PEERS, a file
of "<PeerID> <name>" lines (e.g. from each node's "Local PeerId" log line), replaces it by the
node's name. Lines other than BENCH_RX, such as the publisher's <run>.peer_4 status file, are
skipped.
"""
import csv
import os
import re
import sys

LINE = re.compile(r"BENCH_RX .*?via=(\S+) seq=(\d+) latency_ms=(\d+)")


def main(raw_dir, out, peers=None):
    names = {}
    if peers:
        for line in open(peers):
            if line.strip():
                peer_id, name = line.split()[:2]
                names[peer_id] = name
    files = sorted((f for f in os.listdir(raw_dir) if "." in f),
                   key=lambda f: (os.path.getmtime(os.path.join(raw_dir, f)), f))
    n = 0
    with open(out, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["run", "receiver", "via", "seq", "latency_ms"])
        for f in files:
            run, receiver = f.rsplit(".", 1)
            for line in open(os.path.join(raw_dir, f), errors="replace"):
                m = LINE.search(line)
                if m:
                    via, seq, latency = m.groups()
                    w.writerow([run, receiver, names.get(via, via), seq, latency])
                    n += 1
    print(f"{n} receipts from {len(files)} files written to {out}")


if __name__ == "__main__":
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    main(*sys.argv[1:])
