#!/usr/bin/env python3
"""Collect the single-attacker size series into one CSV, one row per run.

    python3 scripts/size_figure_data.py OUT.csv RESULT_DIR

RESULT_DIR holds one `N*/` folder per network size with the
`experiment_*.csv` files written by dactie-experiment-anonymity.
`direct_publisher_percent2` is the share of the other peers that received the
measurement message directly from its source (`from_publisher / (N - 1)`).
"""
import csv
import glob
import sys


def main(out, result_dir):
    rows = []
    for f in sorted(glob.glob(f"{result_dir}/N*/experiment_[0-9]*.csv")):
        for r in csv.DictReader(open(f)):
            if r["total_peers"].isdigit():
                rows.append(r)
    rows.sort(key=lambda r: int(r["total_peers"]))
    with open(out, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["total_peers", "from_publisher", "relayed", "delivery_ratio",
                    "direct_publisher_percent2", "latency_p50_us", "latency_p95_us"])
        for r in rows:
            n, d = int(r["total_peers"]), int(r["correct_receiver"])
            w.writerow([n, d, r["other_peer"], r["delivery_ratio"], round(d / (n - 1), 10),
                        r["latency_p50_us"], r["latency_p95_us"]])
    print(f"{len(rows)} runs written to {out}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
