#!/usr/bin/env python3
"""Collect the eclipse rerun of 2026-11-15/16 into one CSV and summarize it.

    python3 scripts/parse_eclipse_rerun.py [RESULT_DIR]

RESULT_DIR (default results/eclipse/rerun-2026-11-15) holds
<gossip-on|gossip-off>/<output|old_output>/<attackers>/eclipse_*.csv, written by
experiments/eclipse with the released mesh band (10/12/24), flood publishing off, and gossip
emission on (libp2p's defaults, --gossip-lazy 6 --gossip-factor 0.25) or off (both 0). Two grids
per setting: "small" (output: 10 to 100 attackers, 2 to A/4 honest peers, 3 runs each; 375 runs)
and "large" (old_output: 10 and 30 to 100 attackers, 100 to 120 honest peers, 5 runs each; 945
runs). A cell resumed after an interruption has two CSVs, eclipse_100_120_* with the runs before
it and eclipse_<resume>_120_* with the rest; both are read. The eclipse_avg_*.csv files hold
per-setting averages and *.csv.killed the interrupted originals; neither is read.

Writes RESULT_DIR/eclipse_gossip_parsed.csv, one row per run, and for the paper's figure
eclipse_gossip_on.csv and eclipse_gossip_off.csv.
"""

import csv
import glob
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_DIR = os.path.join(ROOT, "results", "eclipse", "rerun-2026-11-15")
GRIDS = {"output": "small", "old_output": "large"}
HEADER = [
    "total_peers",
    "honest_peers",
    "attacker_peers",
    "attacker_ratio",
    "received_messages",
    "message_ratio",
    "gossip",
    "grid",
]


def collect(result_dir: str) -> list[list[object]]:
    rows: list[list[object]] = []
    for gossip in ("on", "off"):
        for folder, grid in GRIDS.items():
            pattern = os.path.join(result_dir, f"gossip-{gossip}", folder, "*", "eclipse_*.csv")
            for path in sorted(glob.glob(pattern)):
                if os.path.basename(path).startswith("eclipse_avg_"):
                    continue
                with open(path, newline="", encoding="utf-8") as f:
                    for row in csv.DictReader(f):
                        total = int(row["total_peers"])
                        honest = int(row["honest_peers"])
                        attackers = int(row["attacker_peers"])
                        received = int(row["received_messages"])
                        # The victim is one of the honest peers, so honest - 1 peers can receive.
                        rows.append(
                            [
                                total,
                                honest,
                                attackers,
                                attackers / total,
                                received,
                                received / (honest - 1),
                                gossip,
                                grid,
                            ]
                        )
    return rows


def summary(rows: list[list[object]]) -> None:
    for gossip in ("on", "off"):
        for grid in GRIDS.values():
            runs = [r for r in rows if r[6] == gossip and r[7] == grid]
            if not runs:
                continue
            cut_off = [r for r in runs if r[4] == 0]
            partial = [r for r in runs if 0 < r[4] < r[1] - 1]
            line = f"gossip {gossip:3} {grid:5}: {len(runs)} runs, victim cut off in {len(cut_off)}"
            line += f", partly delivered in {len(partial)}"
            if cut_off:
                line += (
                    f"; cut off with {min(r[1] for r in cut_off)}-{max(r[1] for r in cut_off)}"
                    f" honest peers and {min(r[3] for r in cut_off):.0%}-{max(r[3] for r in cut_off):.0%}"
                    " attackers"
                )
            print(line)


def main() -> None:
    result_dir = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_DIR
    rows = collect(result_dir)
    with open(os.path.join(result_dir, "eclipse_gossip_parsed.csv"), "w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(HEADER)
        writer.writerows(rows)
    for gossip in ("on", "off"):
        with open(os.path.join(result_dir, f"eclipse_gossip_{gossip}.csv"), "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(HEADER)
            writer.writerows(r for r in rows if r[6] == gossip)
    print(f"{len(rows)} runs from {result_dir}")
    summary(rows)


if __name__ == "__main__":
    main()
