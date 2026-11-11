# dactie-benchmark

Criterion microbenchmarks for `thressig`, the threshold group signature
(Camenisch et al. 2020) that DACTIE uses to sign messages and to let a threshold
of archives open a signature.

## Run

```bash
cargo bench -p dactie-benchmark --bench thressig
```

Takes about 5 minutes. HTML reports land in `target/criterion/`. Parameters are
environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `DACTIE_BENCH_GROUP_SIZES` | `10,100,500` | member counts for the opening benchmarks |
| `DACTIE_BENCH_ARCHIVES` | `3` | archives (openers) |
| `DACTIE_BENCH_THRESHOLD` | `3` | archives needed to open |
| `DACTIE_BENCH_MSG_BYTES` | `1024` | signed payload size |
| `DACTIE_BENCH_PEERS_PER_MEMBER` | `1` | sharing peers per member for the opening benchmarks; every member has at least the one it joined from |

The defaults match AttriChain's evaluation (3 tracing members, threshold 3).

`cargo test -p dactie-benchmark --release` checks that the fixture signs,
verifies and opens to the right member.

## What is measured

- `per_message/sign`, `per_message/verify`: cost per published / received message.
- `join/full_join`: member side, authority side and share verification for one member.
- `open/archive_partial_open/<n>`: one archive's work for an opening request. The
  signer is unknown, so the archive computes one pairing per stored share: one per member
  and one per registered sharing peer.
- `open/combine/<n>`: the AA combining the partial openings of `t` archives; checks every
  member and every sharing peer, as `open_signature` does.

## Results (2026-10-11, one share per member, before the constants of `open` were hoisted)

Release build, single thread, 3 archives, t = 3, 1 KiB payload. Signature size: 226 bytes.

| Operation | Server (AMD EPYC 7763) | Laptop (Intel Core Ultra 7 165U) |
| --- | --- | --- |
| sign | 2.67 ms | 1.86 ms |
| verify | 7.53 ms | 4.97 ms |
| full join | 55.6 ms | 36.5 ms |
| archive partial open, n = 10 / 100 / 500 | 25.5 ms / 254 ms / 1.26 s | 17.3 ms / 169 ms / 854 ms |
| combine, n = 10 / 100 / 500 | 46.9 ms / 461 ms / 2.30 s | 31.4 ms / 307 ms / 1.56 s |

The paper reports the server numbers (same server as the anonymity experiments).
Opening is linear in the group size: on the server about 2.5 ms per member per archive,
plus about 4.6 ms per member to combine.

For comparison, AttriChain (t = 3, i7-6300U, Solidity on a permissioned Ethereum
instance) reports Sign ≈ 3.2 s, Trace 4.6 s and Collect 5.4 s; Huff and Li report
4.2 s per zk-SNARK proof and 28.5 ms per verification. The hardware differs, so
treat these as order-of-magnitude comparisons.

## Results with sharing peers (2026-10-12, laptop)

Laptop as above, 3 archives, t = 3, 500 members with one sharing peer each (1,000 stored shares):

| Operation | Before hoisting | After hoisting |
| --- | --- | --- |
| archive partial open | 1.69 s | 1.71 s |
| combine | 3.11 s | 0.88 s |

Server (AMD EPYC 7763), 2026-10-13, single core (`taskset -c 4`), after hoisting:

| Operation | n = 10 | n = 100 | n = 500 |
| --- | --- | --- | --- |
| archive partial open | 50.6 ms | 509 ms | 2.58 s |
| combine | 27.2 ms | 207 ms | 1.01 s |

with one sharing peer per member, i.e. 2n stored shares: about 2.6 ms per share per archive
plus 1.0 ms per share to combine, so 3.6 s in total for 500 members. Sign 2.66 ms, verify
7.58 ms, signature 226 bytes.

`thressig::open` used to recompute two pairings and the Lagrange coefficients for every
candidate, although they do not depend on it; it now computes them once. The archive's
work is unchanged and still one pairing per stored share.
