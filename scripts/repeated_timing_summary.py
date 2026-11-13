#!/usr/bin/env python3
"""Summarize results/repeated/ and results/coalition/timing/ as reported in the paper.

Usage: python3 scripts/repeated_timing_summary.py [results]
"""
import csv
import glob
import statistics as st
import sys
from collections import defaultdict

root = sys.argv[1] if len(sys.argv) > 1 else "results"


def load(pattern):
    rows = []
    for f in glob.glob(pattern):
        with open(f) as fh:
            rows += list(csv.DictReader(fh))
    return rows


# Repeated messages: mean over the runs, member-level success.
multi = defaultdict(list)
for r in load(f"{root}/repeated/*/N*/multi_message_*.csv"):
    key = (r["regime"], int(r["total_peers"]), int(r["k"]), int(r["coalition_size"]),
           int(r["m"]), r["estimator"])
    multi[key].append(float(r["success_member"]))
mm = lambda *k: st.mean(multi[k])

print("Repeated messages, k = 1, after m = 10 (best of vote and disclosure) vs 1 - (1 - c/N)^10")
for reg in ("no_stem", "stem_fixed", "stem_rotating"):
    for n in (50, 100, 200):
        for c in (1, 5, 10, 20):
            best = max(mm(reg, n, 1, c, 10, "first_spy_vote"), mm(reg, n, 1, c, 10, "disclosure"))
            eq = 1 - (1 - c / n) ** 10
            print(f"  {reg:13s} N={n:3d} c={c:2d}  m=1 {mm(reg, n, 1, c, 1, 'first_spy'):.3f}"
                  f"  m=10 {best:.3f}  eq {eq:.3f}  ratio {best / eq:.2f}")

print("\nk PeerIDs within one stem epoch (stem_fixed), disclosure after m = 10 vs 1 - (1 - c/N)^k")
for n in (50, 100, 200):
    for c in (5, 10):
        cells = [f"k={k} {mm('stem_fixed', n, k, c, 10, 'disclosure'):.3f}"
                 f" (eq {1 - (1 - c / n) ** k:.3f}, uniform {mm('stem_fixed', n, k, c, 10, 'random'):.3f})"
                 for k in (1, 3, 5)]
        print(f"  N={n:3d} c={c:2d}  " + "  ".join(cells))

# Timing estimators: mean success over the runs.
timing = defaultdict(list)
for r in load(f"{root}/coalition/timing/q*/D*_N*/adversary_*.csv"):
    key = (float(r["stem_q"]), int(r["mesh_n"]), int(r["total_peers"]),
           int(r["coalition_size"]), r["estimator"])
    timing[key].append(float(r["success_rate"]))
tm = lambda *k: st.mean(timing[k])
cells = sorted({k[:4] for k in timing})

print("\nTiming estimators, c = 2 and c = 5")
for q in (0.0, 0.2):
    for d in (6, 8, 10, 12):
        for n in (50, 100, 150, 200):
            print(f"  q={q} D={d:2d} N={n:3d}  " + "  ".join(
                f"c={c}: first_spy {tm(q, d, n, c, 'first_spy'):.3f} timing_delay {tm(q, d, n, c, 'timing_delay'):.3f}"
                f" timing_delay_stem {tm(q, d, n, c, 'timing_delay_stem'):.3f}" for c in (2, 5)))
stem = [c for c in cells if c[0] == 0.2]
diff = [tm(*c, "timing_delay_stem") - tm(*c, "first_spy") for c in stem]
beats = [c for c in stem if c[3] >= 2 and tm(*c, "timing_delay") >= tm(*c, "first_spy")]
print(f"\nq = 0.2: timing_delay_stem - first_spy in [{min(diff):+.3f}, {max(diff):+.3f}];"
      f" cells with c >= 2 where timing_delay >= first_spy: {len(beats)}")
