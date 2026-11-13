"""One row per arrival rate with DACTIE and Fabric side by side, from the summary.csv files of
scripts/rate_sweep.sh (receiver peer_1) and experiments/fabric/sweep.sh (receiver peer0.org1).
This is the data behind the comparison figure in the paper.
Usage: python3 compare.py DACTIE_DIR FABRIC_MISP_DIR [FABRIC_20B_DIR] [NAME=FABRIC_DIR ...] > rate_sweep.csv
Each NAME=FABRIC_DIR adds another Fabric sweep as columns NAME_tps, NAME_ms, NAME_p95,
NAME_loss, e.g. fabricidemix=... for the sweep with anonymous Idemix clients (CLIENT=idemix)."""
import csv
import sys


def load(directory, receiver):
    with open(f'{directory}/summary.csv') as f:
        return {int(r['rate']): r for r in csv.DictReader(f) if r['receiver'] == receiver}


positional = [a for a in sys.argv[1:] if '=' not in a]
named = [a.split('=', 1) for a in sys.argv[1:] if '=' in a]
series = [('dactie', load(positional[0], 'peer_1')), ('fabric', load(positional[1], 'peer0.org1'))]
if len(positional) > 2:
    series.append(('fabric20', load(positional[2], 'peer0.org1')))
series += [(name, load(directory, 'peer0.org1')) for name, directory in named]
rates = sorted(set().union(*(s.keys() for _, s in series)))
w = csv.writer(sys.stdout, lineterminator='\n')
w.writerow(['rate'] + [f'{name}_{col}' for name, _ in series for col in ('tps', 'ms', 'p95', 'loss')])
for rate in rates:
    row = [rate]
    for _, s in series:
        r = s.get(rate)
        row += [r['throughput'], r['mean_ms'], r['p95_ms'], r['loss']] if r else ['nan'] * 4
    w.writerow(row)
