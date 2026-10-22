#!/usr/bin/env python3
"""Collect the single eclipse runs of output/ and old_output/ into eclipse_parsed.csv.

Only the experiment_<min honest>_<max honest>_<stop time>_<interval>.csv files hold single
runs; the experiment_avg_*.csv files next to them hold per-setting averages and are skipped.
"""

import csv
import glob
import os
from typing import List

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
SERIES = ["output", "old_output"]
OUTPUT_CSV = os.path.join(BASE_DIR, "eclipse_parsed.csv")


def collect_rows() -> List[List[object]]:
    """Collect one row per run, with the attacker share and the share of honest receivers."""
    rows: List[List[object]] = []

    for series in SERIES:
        pattern = os.path.join(BASE_DIR, series, "*", "experiment_*.csv")
        for path in sorted(glob.glob(pattern)):
            if os.path.basename(path).startswith("experiment_avg_"):
                continue

            with open(path, newline="", encoding="utf-8") as f:
                for row in csv.DictReader(f):
                    total_peers = int(row["total_peers"])
                    honest_peers = int(row["honest_peers"])
                    attacker_peers = int(row["attacker_peers"])
                    received_messages = int(row["received_messages"])

                    # The victim is one of the honest peers, so honest_peers - 1 peers can receive.
                    received_messages_soll = honest_peers - 1
                    rows.append(
                        [
                            total_peers,
                            honest_peers,
                            attacker_peers,
                            attacker_peers / total_peers,
                            received_messages,
                            received_messages / received_messages_soll,
                            received_messages_soll,
                            series,
                        ]
                    )

    return rows


def main() -> None:
    rows = collect_rows()

    header = [
        "total_peers",
        "honest_peers",
        "attacker_peers",
        "attacker_ratio",
        "received_messages",
        "message_ratio",
        "received_messages_soll",
        "series",
    ]

    with open(OUTPUT_CSV, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(header)
        writer.writerows(rows)

    eclipsed = [r for r in rows if r[4] == 0]
    print(f"Wrote {len(rows)} runs to {OUTPUT_CSV}")
    for series in SERIES:
        print(f"  {series}: {sum(1 for r in rows if r[7] == series)} runs")
    print(f"  successful eclipses: {len(eclipsed)}")


if __name__ == "__main__":
    main()
