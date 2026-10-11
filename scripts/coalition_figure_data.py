#!/usr/bin/env python3
"""Aggregate coalition-attack results into the CSV behind the paper's coalition figure.

    python3 scripts/coalition_figure_data.py OUT.csv RESULT_DIR [RESULT_DIR ...]

Each RESULT_DIR holds `adversary_*_D*_Fix(*).csv` files written by
dactie-experiment-anonymity. The paper uses two independent v3 runs with
identical parameters (the evaluation server and a laptop), so every
(N, D, c) cell averages ten runs. Rows with incomplete delivery or N > 200 are
dropped. `bound_high` is Eq. (psml-coalition) evaluated at D = mesh_n_high.
"""
import collections
import csv
import glob
import statistics as st
import sys


def bound(n, d, c):
    """P_SML(c) = P_adj(c) + (1 - P_adj(c)) / n for a coalition of size c."""
    p = 1.0
    for k in range(1, c + 1):
        p *= max(n - k - d, 0) / (n - k)
    p_adj = 1 - p
    return p_adj + (1 - p_adj) / n


def main(out, dirs):
    cell = collections.defaultdict(list)
    for d in dirs:
        for f in sorted(glob.glob(f"{d}/adversary_*.csv")):
            for r in csv.DictReader(open(f)):
                if int(r["total_peers"]) > 200:
                    continue
                if r.get("delivery_ratio") and float(r["delivery_ratio"]) < 1:
                    continue
                key = (int(r["total_peers"]), int(r["mesh_n"]), int(r["mesh_n_high"]),
                       int(r["coalition_size"]), r["estimator"])
                cell[key].append(float(r["success_rate"]))
    configs = sorted({k[:4] for k in cell})
    with open(out, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["N", "D", "Dhigh", "c", "f", "first_spy", "random", "runs", "bound_high", "ratio"])
        for n, d, dh, c in configs:
            fs = cell[(n, d, dh, c, "first_spy")]
            rd = cell[(n, d, dh, c, "random")]
            b = bound(n, dh, c)
            w.writerow([n, d, dh, c, round(c / n, 4), round(st.mean(fs), 5), round(st.mean(rd), 5),
                        len(fs), round(b, 5), round(st.mean(fs) / b, 4)])
    print(f"{len(configs)} configurations written to {out}")


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2:])
