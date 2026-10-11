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

The defaults match AttriChain's evaluation (3 tracing members, threshold 3).

`cargo test -p dactie-benchmark --release` checks that the fixture signs,
verifies and opens to the right member.

## What is measured

- `per_message/sign`, `per_message/verify`: cost per published / received message.
- `join/full_join`: member side, authority side and share verification for one member.
- `open/archive_partial_open/<n>`: one archive's work for an opening request. The
  signer is unknown, so the archive computes one pairing per member.
- `open/combine/<n>`: combining the partial openings of `t` archives; checks every member.

## Results (2026-10-11)

Intel Core Ultra 7 165U, 30 GiB RAM, release build, single thread, 3 archives, t = 3, 1 KiB payload.
Signature size: 226 bytes.

| Operation | n = 10 | n = 100 | n = 500 |
| --- | --- | --- | --- |
| sign | 1.86 ms | | |
| verify | 4.97 ms | | |
| full join | 36.5 ms | | |
| archive partial open | 17.3 ms | 169 ms | 854 ms |
| combine | 31.4 ms | 307 ms | 1.56 s |

Opening is linear in the group size: about 1.7 ms per member per archive, plus
about 3.1 ms per member to combine.

For comparison, AttriChain (t = 3, i7-6300U, Solidity on a permissioned Ethereum
instance) reports Sign ≈ 3.2 s, Trace 4.6 s and Collect 5.4 s; Huff and Li report
4.2 s per zk-SNARK proof and 28.5 ms per verification. The hardware differs, so
treat these as order-of-magnitude comparisons.

## Note

`thressig::open` recomputes two pairings and the Lagrange coefficients for every
member, although they do not depend on the member. Hoisting them out of the loop
would save two pairings per member in the combine step (not measured). The benchmark measures the code as used
by DACTIE and leaves `thressig` unchanged.
