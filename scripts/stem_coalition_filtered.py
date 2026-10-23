"""Stem coalition results without the runs whose measurement message skipped the stem
(startup bug: the source's epoch began without successors; fixed in ce18a68).

    python3 scripts/stem_coalition_filtered.py DIR

DIR holds publish_methods.txt ("q0.2/D12_N200 <run> <stem|ok>", from each cell's run.log)
and stem2/q*/D*_N*/adversary_*.csv as in ~/anonymity-results-stem2/coalition on the server.
Writes DIR/filtered_q*.csv in the format of coalition_figure_data.py."""
import csv, glob, os, sys, collections, statistics as st
S = sys.argv[1]
bad = {(l.split()[0], l.split()[1]) for l in open(f"{S}/publish_methods.txt") if not l.strip().endswith(" stem")}
def bound(n, d, c):
    p = 1.0
    for k in range(1, c + 1):
        p *= max(n - k - d, 0) / (n - k)
    return (1 - p) + p / n
out = {}
for q in ("0.1", "0.2", "0.5"):
    cell = collections.defaultdict(list)
    runs = collections.defaultdict(set)
    for f in glob.glob(f"{S}/stem2/q{q}/D*_N*/adversary_*.csv"):
        cname = f"q{q}/" + os.path.basename(os.path.dirname(f))
        for r in csv.DictReader(open(f)):
            if (cname, r["run"]) in bad:
                continue
            key = (int(r["total_peers"]), int(r["mesh_n"]), int(r["mesh_n_high"]), int(r["coalition_size"]))
            cell[key + (r["estimator"],)].append(float(r["success_rate"]))
            runs[key].add(r["run"])
    rows = []
    for key in sorted(runs):
        n, d, dh, c = key
        fs = st.mean(cell[key + ("first_spy",)]); rnd = st.mean(cell[key + ("random",)])
        b = bound(n, dh, c)
        rows.append(dict(N=n, D=d, Dhigh=dh, c=c, f=round(c / n, 4), first_spy=round(fs, 5), random=round(rnd, 5),
                         runs=len(runs[key]), bound_high=round(b, 5), ratio=round(fs / b, 4)))
    with open(f"{S}/filtered_q{q}.csv", "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=list(rows[0])); w.writeheader(); w.writerows(rows)
    ratio = [r["first_spy"] / (r["c"] / r["N"]) for r in rows]
    above = sum(r["first_spy"] >= r["bound_high"] for r in rows)
    d12 = {r["c"]: r["first_spy"] for r in rows if r["N"] == 200 and r["D"] == 12}
    excl = sum(1 for b in bad if b[0].startswith(f"q{q}/"))
    print(f"q={q}: {len(rows)} configs, {excl} runs excluded; mean {st.mean(ratio):.2f}x c/N (max {max(ratio):.2f}), >= bound {above}; D12 N200 c1/5/20 = {d12[1]:.4f}/{d12[5]:.4f}/{d12[20]:.4f}")
