#!/usr/bin/env python3
"""Split eclipse runs by gossip setting into the paper's two figure series and summarize them.

    python3 scripts/parse_eclipse_rerun.py INPUT [OUT_DIR]

INPUT is either
- the results CSV that experiments/eclipse appends one row per run to (--results, by default
  <out-path>/eclipse_results.csv), or
- a folder with the per-series CSVs of harness versions before that CSV, such as the rerun of
  2026-11-15/16: <gossip-on|gossip-off>/<output|old_output>/<attackers>/eclipse_*.csv, i.e. each
  series run with -o INPUT/gossip-<on|off>/<grid>/<attackers>/, the grid "small" (output: 2 to
  A/4 honest peers) or "large" (old_output: 100 to 120 honest peers). A series resumed after an
  interruption has one CSV per part; all are read. eclipse_avg_*.csv files hold per-setting
  averages and are skipped.

Gossip emission counts as on unless --gossip-lazy and --gossip-factor were both 0. Writes
eclipse_gossip_on.csv and eclipse_gossip_off.csv, and for a folder also eclipse_gossip_parsed.csv
with every run, to OUT_DIR (default: the folder of the results CSV, or the folder itself).
experiments/eclipse/README.md lists the flags of the rerun; its parsed runs are in
results/eclipse/rerun-2026-11-15/.
"""

import csv
import glob
import os
import sys

GRIDS = {"output": "small", "old_output": "large"}
FOLDER_FIELDS = [
    "total_peers",
    "honest_peers",
    "attacker_peers",
    "attacker_ratio",
    "received_messages",
    "message_ratio",
    "gossip",
    "grid",
]


def from_folder(raw_dir: str) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    for gossip in ("on", "off"):
        for folder, grid in GRIDS.items():
            pattern = os.path.join(raw_dir, f"gossip-{gossip}", folder, "*", "eclipse_*.csv")
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
                            {
                                "total_peers": total,
                                "honest_peers": honest,
                                "attacker_peers": attackers,
                                "attacker_ratio": attackers / total,
                                "received_messages": received,
                                "message_ratio": received / (honest - 1),
                                "gossip": gossip,
                                "grid": grid,
                            }
                        )
    return rows


def from_results(path: str) -> tuple[list[dict[str, object]], list[str]]:
    with open(path, newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        rows: list[dict[str, object]] = []
        for row in reader:
            emission = float(row["gossip_lazy"]) > 0 or float(row["gossip_factor"]) > 0
            rows.append({**row, "gossip": "on" if emission else "off"})
        fields = [*(reader.fieldnames or []), "gossip"]
    return rows, fields


def summary(rows: list[dict[str, object]]) -> None:
    for gossip in ("on", "off"):
        for grid in (*GRIDS.values(), None):
            runs = [r for r in rows if r["gossip"] == gossip and r.get("grid") == grid]
            if not runs:
                continue
            honest = [int(str(r["honest_peers"])) for r in runs]
            received = [int(str(r["received_messages"])) for r in runs]
            cut_off = [i for i, n in enumerate(received) if n == 0]
            partial = [i for i, n in enumerate(received) if 0 < n < honest[i] - 1]
            label = f"gossip {gossip:3} {grid:5}" if grid else f"gossip {gossip:3}"
            line = f"{label}: {len(runs)} runs, victim cut off in {len(cut_off)}"
            line += f", partly delivered in {len(partial)}"
            if cut_off:
                share = [float(str(runs[i]["attacker_ratio"])) for i in cut_off]
                line += (
                    f"; cut off with {min(honest[i] for i in cut_off)}-{max(honest[i] for i in cut_off)}"
                    f" honest peers and {min(share):.0%}-{max(share):.0%} attackers"
                )
            print(line)


def write(path: str, fields: list[str], rows: list[dict[str, object]]) -> None:
    with open(path, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


def main() -> None:
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    source = sys.argv[1]
    if not os.path.exists(source):
        sys.exit(f"{source} does not exist")
    if os.path.isdir(source):
        rows, fields, default_out = from_folder(source), FOLDER_FIELDS, source
    else:
        rows, fields = from_results(source)
        default_out = os.path.dirname(os.path.abspath(source))
    out_dir = sys.argv[2] if len(sys.argv) == 3 else default_out
    if not rows:
        sys.exit(f"no runs in {source}; nothing written")
    os.makedirs(out_dir, exist_ok=True)
    if os.path.isdir(source):
        write(os.path.join(out_dir, "eclipse_gossip_parsed.csv"), fields, rows)
    for gossip in ("on", "off"):
        part = [r for r in rows if r["gossip"] == gossip]
        write(os.path.join(out_dir, f"eclipse_gossip_{gossip}.csv"), fields, part)
    print(f"{len(rows)} runs from {source}, written to {out_dir}")
    summary(rows)


if __name__ == "__main__":
    main()
