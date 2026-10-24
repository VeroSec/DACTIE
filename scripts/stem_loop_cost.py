"""Cost of the source's loop re-send in the Dandelion++ stem, replaying the harness routing.

Usage: python3 scripts/stem_loop_cost.py <graphs> <messages per graph>  (the published
numbers used 100 500). Rules for the source's own message when the stem brings it back: `back` to the
successor that already has it, `random2` one of its two successors (the sharing peer before the
loop-rule decision), `fresh` the other successor (the harness), `randconn` a random connection.
Graphs follow the harness: 50 to 100 connections per peer, two successors per epoch, one fixed
successor per predecessor, a relay that sees a message again publishes it.

Per message: the stem path as a list of (receiver, predecessor). Stem-aware first-spy names the
predecessor of the earliest colluding receiver; it succeeds iff that predecessor is the source.
The success probability over a uniformly random coalition of size c (source excluded) is computed
exactly per path (hypergeometric), so only the paths are sampled.
"""
import random, sys
from math import comb

def graph(n, rng, low=50, high=100):
    nb = [set() for _ in range(n)]
    order = list(range(n)); rng.shuffle(order)
    for v in order:
        tries = 0
        while len(nb[v]) < min(low, n - 1) and tries < 10 * n:
            u = rng.randrange(n); tries += 1
            if u != v and u not in nb[v] and len(nb[u]) < high:
                nb[v].add(u); nb[u].add(v)
    return [list(s) for s in nb]

def path(src, nb, q, rule, rng, state):
    def st(v):
        if v not in state:
            succ = rng.sample(nb[v], min(2, len(nb[v])))
            state[v] = dict(diff=rng.random() < q, succ=succ, own=rng.choice(succ) if succ else None, routes={})
        return state[v]
    seen = {src}; receipts = []; retries = 0
    s = st(src); hop = (s['own'], src)       # (next receiver, its predecessor)
    while hop[0] is not None:
        r, p = hop
        receipts.append((r, p))
        if r == src:                          # own message came back
            if retries >= 3: break            # left to the embargo
            retries += 1
            if rule == 'back':
                nxt = s['own']
            elif rule == 'random2':
                nxt = rng.choice(s['succ'])
            else:
                avoid = {p, s['own']}
                pref = [x for x in s['succ'] if x not in avoid]
                if rule == 'fresh' and pref: nxt = rng.choice(pref)
                else:
                    spare = [x for x in nb[src] if x not in avoid]
                    nxt = rng.choice(spare) if spare else None
            hop = (nxt, src); continue
        if r in seen: break                   # relay loop: it fluffs
        seen.add(r)
        t = st(r)
        if t['diff']: break                   # diffuser fluffs
        if p not in t['routes']:
            opts = [x for x in t['succ'] if x != p]
            t['routes'][p] = rng.choice(opts) if opts else None
        nxt = t['routes'][p]
        if nxt is None: break
        hop = (nxt, r)
    return receipts

def success(receipts, src, n, c):
    m = n - 1; total = comb(m, c); earlier = set(); p = 0.0
    for r, pred in receipts:
        if r == src or r in earlier: continue
        if pred == src:
            p += comb(m - len(earlier) - 1, c - 1) / total
        earlier.add(r)
    return p

def run(n, q, rule, graphs, msgs, seed):
    rng = random.Random(seed); cs = (1, 5, 20)
    acc = {c: 0.0 for c in cs}; loops = 0; firsthops = 0; k = 0
    for _ in range(graphs):
        nb = graph(n, rng)
        for _ in range(msgs):
            state = {}                        # fresh epoch per message
            src = rng.randrange(n)
            rc = path(src, nb, q, rule, rng, state)
            loops += any(r == src for r, _ in rc)
            firsthops += len({r for r, p in rc if p == src and r != src})
            for c in cs: acc[c] += success(rc, src, n, c)
            k += 1
    return loops / k, firsthops / k, {c: acc[c] / k for c in cs}

if __name__ == '__main__':
    graphs, msgs = int(sys.argv[1]), int(sys.argv[2])
    print("N,q,rule,P_loop,first_hops,c1,c5,c20,c1_rel,c5_rel,c20_rel")
    for n in (50, 100, 200):
        for q in (0.1, 0.2, 0.5):
            for rule in ('back', 'random2', 'fresh', 'randconn'):
                pl, fh, s = run(n, q, rule, graphs, msgs, seed=hash((n, q)) & 0xffff)
                rel = {c: s[c] / (c / (n - 1)) for c in s}
                print(f"{n},{q},{rule},{pl:.4f},{fh:.4f},{s[1]:.5f},{s[5]:.5f},{s[20]:.5f},{rel[1]:.3f},{rel[5]:.3f},{rel[20]:.3f}", flush=True)
