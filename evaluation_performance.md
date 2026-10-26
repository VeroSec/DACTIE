# Performance evaluation — supporting detail

Backing material for the *Evaluation Performance* paragraph of the rebuttal, which is
held to roughly 200 words. Everything here is measured on the four-server testbed;
nothing is extrapolated or emulated.

## Testbed

Four hosts on a switched LAN, one service role per host except where noted:

| Host | Services | Ports |
|---|---|---|
| A (`.116`) | Authorization Authority | 10001 |
| B (`.117`) | PostgreSQL, Archive 1, peer_1 | 5432, 20011, 21011 |
| C (`.118`) | PostgreSQL, Archive 2, peer_2 | 5433, 20012, 21012 |
| D (`.119`) | peer_3, peer_4 | 21013, 21014 |

peer_4 is the publisher throughout. It never appears in receiver tables: GossipSub does
not deliver a publisher its own message, so peer_4 has no receive latency to measure. Over
the soak it logged 597 507 publications and zero receipts.

## Latency by payload, MISP and STIX

40 msg/s, the rate reported by [52], over the full encrypted path (group
signature, MLS encryption, GossipSub). Measured end-to-end from publication to
application delivery at a peer on another host. Each row is 800–1000 deliveries.

| Payload | MISP on wire | MISP mean / p95 | STIX on wire | STIX mean / p95 |
|---|---|---|---|---|
| 1 KiB | 1 169 B | 21.9 / 33 ms | 1 184 B | 22.0 / 35 ms |
| 4 KiB | 4 482 B | 21.2 / 29 ms | 4 380 B | 21.0 / 28 ms |
| 16 KiB | 16 664 B | 22.1 / 30 ms | 16 306 B | 21.6 / 30 ms |
| 64 KiB | 65 811 B | 23.6 / 33 ms | 65 743 B | 23.3 / 32 ms |
| 256 KiB | 262 431 B | 26.3 / 36 ms | 261 855 B | 26.5 / 36 ms |
| full event / full feed | 2 362 522 B | 89.8 / 99 ms | 2 951 973 B | 102.2 / 113 ms |

The last row is one uncut MISP event and the complete MITRE ATT&CK ICS domain feed
(2 173 STIX 2.1 objects), published at 5 msg/s because 40 msg/s of 2.9 MB exceeds the
link, not the system.

Two results carry the argument:

- Latency is flat in payload size across a 256-fold increase (21.9 → 26.3 ms), so cost is
  dominated by constant per-message work rather than transport.
- MISP and STIX are indistinguishable at every size. The result is therefore a property of
  the system, not of one CTI encoding, and generalises to whichever format a deployment
  uses.

STIX bundles are built from the ATT&CK ICS feed by packing whole objects until just under
each target size; a bundle truncated mid-object does not parse, and an unparseable payload
measures nothing. `relationship` objects are packed last — they are 1 667 of the 2 173
objects but carry little substantive content, so a small bundle made only of them would
not resemble what an organisation shares. See `stix/NOTICE.md` for provenance and the
MITRE licence terms; `scripts/make_stix_payloads.py` regenerates them.

## Two implementation changes this required

**GossipSub `max_transmit_size`.** The default is 65 536 bytes. A 64 KiB MISP event is
65 811 bytes on the wire once the group signature and framing are added, so every payload
at or above 64 KiB was rejected before delivery, silently. Large threat feeds are not an
incidental case for a CTI system; the limit is now raised at peer, archive and authority,
which must agree or a receiver drops the message rather than reporting a size error.

**Signature encoding.** `amcl_wrapper` serialises G1 and field elements as hex strings.
Replacing this with the compact byte form takes a signature from 614 to 226 bytes on the
wire (2.7×). This is a wire-format change: all nodes must run a matching build.

## Sustained operation: 33.4 hours

The sampler records a 60 s window and then sleeps 60 s, so one loop takes ~62.4 s. The
1 925 samples therefore span 33.4 h (2026-08-22 12:41 to 2026-08-23 22:04 UTC), not
32.1 h; the rebuttal's 31.1 h is also too short. "Minutes" below means sampled windows.

A real 16 KiB MISP event at 40 msg/s, sampled once a minute at each receiver.

| Receiver | mean | sd | first 10 min | last 10 min | msg/min | RSS start → end | minutes without delivery |
|---|---|---|---|---|---|---|---|
| peer_1 (host B) | 21.97 ms | 0.36 | 22.25 | 21.90 | 2 420 | 11 → 13 MB | 0 of 1 925 |
| peer_2 (host C) | 21.87 ms | 0.39 | 22.63 | 21.62 | 2 420 | 13 → 15 MB | 0 of 1 925 |
| peer_3 (host D) | 25.56 ms | 0.11 | 25.62 | 25.61 | 2 420 | 12 → 12 MB | 0 of 1 926 |

**13 979 188 sampled deliveries.** No latency drift — all three receivers end *faster*
than they start. Memory growth ≤ 2 MB. No connection churn. No minute in which delivery
was interrupted.

Sustained receipt of 2 420 messages per minute against a publication rate of 2 400 is
consistent with complete delivery. Per-message accounting was not instrumented, so the
claim is *no sustained shortfall*, not zero loss.

Raw series: `soak_results/peer_{1,2,3}.csv`.

## Why peer_3's mean is higher: hop count, not host

peer_3's 25.56 ms sits ~3.6 ms above the other two, and its variance is *lower*
(sd 0.11 vs 0.36). That rules out CPU contention on the shared host, which would add
jitter. Splitting each receiver's own receipts by whether the message arrived directly
from the publisher or via another peer, over 9 999 receipts each:

| Receiver | direct share | direct mean | relayed share | relayed mean | hop cost |
|---|---|---|---|---|---|
| peer_1 | 99.5 % | 21.82 ms | 0.5 % | 31.70 ms | 9.88 ms |
| peer_2 | 99.6 % | 21.57 ms | 0.4 % | 31.57 ms | 10.00 ms |
| peer_3 | 52.0 % | 21.25 ms | 48.0 % | 30.87 ms | 9.62 ms |

Direct latency is effectively identical on all three hosts, and so is relayed latency.
**One additional hop costs ~9.8 ms**, and peer_3's higher mean is entirely a 50/50 path
mix rather than any host-level penalty. Its low variance follows: a stable blend of two
tight modes.

This is worth carrying into the sender-anonymity discussion. With four peers and
`mesh_n = 12` every peer should be meshed with every other, so a first-hop delivery to
peer_3 should always win — yet it does so only half the time, even though peer_3 shares a
host with the publisher. That is the same mesh-versus-gossip interaction described under
*gossip inflates direct exposure*, visible here in the latency data, and it bears on which
peers count as a publisher's direct contacts.

## Archive queries

A peer requests the group commits and proposals newer than its epoch (`RequestMessage::Update`);
the Archive answers from PostgreSQL. Measured across hosts at 2 queries/s:

| Condition | n | mean | p50 | p95 | max |
|---|---|---|---|---|---|
| Quiet network | 82 | 105.1 ms | 106 | 110 | 111 |
| Under 40 msg/s broadcast load | 100 | 108.6 ms | 104 | 151 | 172 |

Query cost is roughly four times dissemination latency and largely insensitive to
concurrent traffic; the tail widens (p95 110 → 151 ms) while the mean moves 3.5 ms.

Two qualifications. These queries return an empty result set, so they measure the round
trip rather than transfer of a backlog. Group-signature verification does **not** explain the gap to the 22 ms dissemination
path: the `benchmark/` crate measures one Cam20 verification at 7.53 ms on the EPYC server
7763 and 4.97 ms on a Core Ultra 7 165U (single thread), so it would take 14 to 20
verifications per query to fill 105 ms. An empty-result query verifies at most one signature. The remaining
~100 ms is the database round trip and request handling, which has not been profiled.

A setup trap worth recording: `move_to_common` removes the common group from `group_ids`,
so a peer holding only the common group has nothing to query and `update_groups` silently
sends zero requests. A private group must exist first.

## On the indirect Hyperledger Fabric comparison

The comparison is indirect by necessity. Thakkar et al. characterise Hyperledger Fabric,
not a CTI-sharing system, and we are aware of no deployed anonymous CTI-exchange prototype
to measure against — the schemes closest in intent (PRACIS; the Dunnet et al. design) are
not available as running systems. What is taken from Thakkar et al. is the *workload rate*,
not a performance claim.

## What is not claimed

- 33.4 hours on four hosts is a soak test, not operational experience. It shows stability
  under sustained realistic load and nothing about deployment in an organisation.
- Four peers is not a scale claim. Dissemination results at larger network sizes remain
  the emulated ones.
- Delivery is reported as no sustained shortfall, not zero loss (see above).

## Re-measurement 2026-10-22: Nagle off, and the stem

Same testbed and payloads, current code (Nagle off on all nodes, the stem on by
default but off at the publisher, `STEM_Q=0`). Receiver `peer_1` on another host, 1,000
deliveries per row after dropping the first 2 s. Clocks within 0.06 ms (chrony).

| Payload | MISP mean / p95 | STIX mean / p95 | before (MISP / STIX) |
|---|---|---|---|
| 1 KiB | 18.8 / 27 ms | 18.6 / 27 ms | 21.9 / 33, 22.0 / 35 |
| 4 KiB | 19.1 / 28 ms | 19.3 / 28 ms | 21.2 / 29, 21.0 / 28 |
| 16 KiB | 19.8 / 28 ms | 19.8 / 28 ms | 22.1 / 30, 21.6 / 30 |
| 64 KiB | 21.7 / 30 ms | 21.3 / 29 ms | 23.6 / 33, 23.3 / 32 |
| 256 KiB | 24.4 / 32 ms | 24.5 / 31 ms | 26.3 / 36, 26.5 / 36 |
| Complete event / feed (5 msg/s) | 104.5 / 117 ms | 116.9 / 133 ms | 89.8 / 99, 102.2 / 113 |

**Nagle on vs off, same build and day** (`--tcp-nodelay`), mean / p95 in ms:

| Run | peer_1 | peer_2 | peer_3 (publisher's host) |
|---|---|---|---|
| 16 KiB, Nagle off | 19.8 / 28 | 20.0 / 28 | 19.3 / 27 |
| 16 KiB, Nagle on | 20.0 / 28 | 19.8 / 28 | 25.9 / 39 |
| complete MISP event, off / on | 93.3 / 99.1 | 86.3 / 101.3 | 88.9 / 112.4 |
| complete ATT&CK feed, off / on | 115.6 / 116.1 | 115.6 / 116.4 | 108.8 / 129.2 |

Nagle does not delay direct deliveries across hosts. It delays relayed ones: `peer_3` gets
about half its copies via another peer, and with Nagle on it shows the 25.6 ms of the soak
test above; with Nagle off it matches the others. The 2–3 ms gain over the old table
therefore comes from other changes since then, not from Nagle. The complete event and feed
vary by up to 11 ms between runs (MISP 104.5 and 93.3 ms with Nagle off), and are not slower
because of Nagle.

**Stem on the publisher** (`STEM_Q=0.2`): 16 KiB MISP events took 80.1 / 104 ms at
`peer_1`, 65.3 / 84 ms at `peer_2` and 78.7 / 104 ms at `peer_3`, against about 20 ms without
the stem, in this four-peer network. Two earlier stem runs were invalid and taught two fixes:
the publisher starts sending right after a restart, before identify reports stem peers, and
kept its empty first epoch for 10 minutes (now topped up); and the relay's limit of 20 stem
messages per second and predecessor dropped half of the 40 msg/s (now 100/s).

Data: `results/latency/rerun-2026-10-22.csv`.

## Re-measurement 2026-10-26: the released code (the paper's table)

The raw logs behind the first table above were not kept, so the paper's table comes from this
run: the released code on the same testbed, RUNBOOK Phases 2–8 from wiped state, the
publisher without the stem, 1,000 deliveries per row at `peer_1` after dropping the first 2 s,
first with Nagle off (the default), then with Nagle on (`TCP_NODELAY=false`, as libp2p
defaults and as in the first table). Mean / p95 in ms:

| Payload | MISP, off | STIX, off | MISP, on | STIX, on |
|---|---|---|---|---|
| 1 KiB | 19.1 / 27 | 19.2 / 27 | 20.9 / 34 | 22.0 / 36 |
| 4 KiB | 19.5 / 27 | 19.5 / 28 | 19.4 / 27 | 19.2 / 27 |
| 16 KiB | 20.4 / 28 | 20.3 / 28 | 20.1 / 28 | 19.6 / 28 |
| 64 KiB | 21.9 / 31 | 21.6 / 30 | 22.0 / 31 | 21.9 / 30 |
| 256 KiB | 25.5 / 33 | 25.3 / 33 | 25.6 / 35 | 25.5 / 34 |
| Complete event / feed (5 msg/s) | 104.4 / 118 | 117.5 / 134 | 102.0 / 115 | 116.7 / 132 |

With Nagle off it matches the run of 2026-10-22 within 1 ms (complete event and feed 104.4 and
117.5 against 104.5 and 116.9 ms). The 1–256 KiB rows stay 1–3 ms below the first table; the
complete event and feed are 13–15 ms slower than there, with Nagle on as well as off, so that
difference does not come from Nagle. `peer_2` (19.1–25.6 ms) and `peer_3` (18.3–25.5 ms) agree
with `peer_1` for 1–256 KiB.

Data: `results/latency/rerun-2026-10-26.csv` (all runs and receivers),
`results/latency/raw-2026-10-26.csv` (every receipt), `scripts/latency_sweep.sh`,
`scripts/latency_summary.py`.

## Reproducing

```bash
./build.sh --no-cache                      # peer image is built with --features bench
# bring the testbed up per RUNBOOK.md, then on the publisher host:
BENCH_PAYLOAD_FILE=/usr/src/dactie/misp/<event>.json BENCH_RATE=40 \
  docker compose --profile node119 up -d --force-recreate peer_4
./scripts/soak_sampler.sh <profile> <service> out.csv   # one row per minute
```

The whole latency table, from a laptop with ssh access to the testbed:
`scripts/latency_sweep.sh OUT_DIR`, about 45 minutes.

Archive queries: `BENCH_ARCHIVE_QUERY=true BENCH_ARCHIVE_PEER=<archive-peer-id> BENCH_ARCHIVE_RATE=2`.
