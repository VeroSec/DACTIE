# Stem phase against coalition first-spy (Dandelion++ for DACTIE)

Status: designed 2026-10-13; implemented in the anonymity harness (branch `feat/stem`) and in
the sharing peer (branch `feat/stem-peer`), not merged. Results and open questions:
`docs/FUTURE_WORK.md`, item 3.1, and "Implementation notes" below.

Parameter sources: BIP 156 (fluff 10%, routes rebuilt every ten minutes on average, random
embargo), Monero `src/cryptonote_config.h` (2 stems, fluff 20%, epoch 10 min + 30 s, embargo
average 39 s), Grin `pool/src/types.rs` (epoch 600 s, stem 90%, embargo 180 s).

## Problem

A sharing peer publishes a broadcast by handing it to its GossipSub mesh peers directly
(10 to 24 of them with the deployed band 10/12/24). A coalition that holds even one of those
mesh slots receives the message from the source first and names it with the first-spy
estimator. With simulated link latency (coalition data v6, D = 12, |N_c| = 200), first-spy
succeeds against a single attacker in 5.2% of cases (uniform guess: 0.5%), against 5 colluders
in 22% and against 20 colluders in 63%. The bound P_SML(c) holds, but it is high, because it is
driven by the source's direct adjacency to D peers.

## Idea

Before a message is spread, it travels along a short random path, the stem, one peer at a time.
Only the peer at the end of the stem, the diffuser, publishes it into GossipSub (the fluff).
Colluders in the mesh then see the diffuser, not the source. To name the source, the coalition
must hold the first stem hop, so first-spy success is expected to drop to roughly the colluding
fraction c/N: from 5.2% to about 0.5% for one attacker and from 63% to about 10% for 20 of 200
peers (estimate from a one-hop stem; measured at q = 0.2: 0.5% and 9.8%). This is Dandelion++ (Fanti et al. 2018,
cited in the paper as future work), adapted to a permissioned GossipSub network.

## Protocol

**Epochs.** Every sharing peer keeps a local stem epoch of 10 minutes plus up to 30 s of
random jitter, so peers do not all switch at once (independent of the registration epoch
`epoch_secs`). At the start of each epoch it:

1. picks two stem successors uniformly at random among its connected, admitted peers
   (`admission::is_admitted`), so the stem graph is roughly 4-regular;
2. becomes a diffuser with probability q (default 0.2 as in Monero, expected stem length
   1/q = 5 hops; BIP 156 and Grin use 0.1), otherwise a relay;
3. as a relay, fixes a one-to-one mapping from each stem predecessor to one of its two
   successors, and sends its own messages always to the same successor. Fixed routes within
   an epoch keep repeated messages of one source from revealing it through intersection.

**Stem forwarding.** A new libp2p request-response protocol `/dactie/stem/1` carries the
signed, MLS-encrypted message exactly as it would be published (the bytes `send_signed`
hands to GossipSub today) plus its topic. On receipt, a peer checks the group signature,
drops duplicates (content-addressed message ID, as in GossipSub), and then either forwards it
to its mapped successor (relay) or publishes it with `gossipsub.publish` (diffuser). The
request is answered with an empty acknowledgement.

**Fail-safe.** Every peer that forwards a stem message starts an embargo timer, exponentially
distributed with a mean of 39 s as in Monero (Grin: 180 s). If the message has not arrived
through GossipSub when the timer fires, the peer publishes it itself. This bounds the damage of
relays that drop messages; GossipSub's duplicate detection absorbs the extra copies. The timer
must be long against a normal stem: a relay that fluffs before the diffuser reveals its stem
position.

**Unchanged.** MLS encryption, group signatures, openings, archives (they receive the message
at the fluff like any other peer), the admission gate and the connection cap. Attributed
messages reveal their MemberID anyway and may skip the stem.

**Abuse.** Only admitted peers are accepted as stem predecessors, every stem message carries a
valid group signature, and each predecessor gets a rate limit, so the stem cannot be used to
inject unsigned traffic or to amplify floods.

## Measurement (anonymity harness)

1. The experiment peer gets the same stem protocol, a `--stem-fluff-probability q` flag
   (0 = off, today's behaviour) and per-epoch routing as above.
2. Peers record stem receipts as observations too: `(observer, predecessor, time)` with a
   flag for stem or gossip. An analysis that logs only GossipSub receipts overstates the anonymity of the stem, because it never counts a colluder on the stem.
3. The first-spy estimator becomes stem-aware: if a colluder received the message on the stem,
   the coalition names the predecessor of the earliest such colluder; otherwise it falls back
   to first-spy on the gossip observations. The other estimators stay as they are.
4. Metrics per configuration: first-spy success, delivery ratio, stem length distribution,
   latency p50/p95 (each stem hop adds one simulated link delay, about 45 ms on average).

**Experiments.** The v6/v7 coalition grid (D in {6, 8, 10, 12}, |N_c| in {50, 100, 150, 200},
5 runs, the same coalition sizes) with q in {0.1, 0.2, 0.5}, compared against q = 0. Later: a
source publishing repeatedly within one epoch, to check the intersection defence.

## Test plan

1. Unit tests: routing table (one-to-one mapping, own messages on one successor), diffuser
   draw, fail-safe timer, duplicate handling.
2. Adversary unit tests: a colluder at stem hop 1 names the source; a colluder at hop k names
   its predecessor, which is wrong; without colluders on the stem, gossip first-spy applies.
3. Harness check at 100 peers: 100% delivery, stem lengths geometric with mean about 1/q.
4. DACTIE end-to-end (`scripts/e2e_local.sh`, `e2e_expiry.sh`) with the stem on: broadcasts
   reach all registered peers and the archives; the unregistered observer still gets nothing.
5. The coalition grid described above.

## Effort

About 2 days for the harness and the adversary model, 2 to 3 days for the sharing peer,
1 to 2 days of experiments.

## Limitations

- The first stem hop learns the source's PeerID if it colludes. Reaching that hop through an
  onion service (the Tor mode) removes this, and is the natural next step.
- Each stem hop adds latency: at q = 0.2 the harness measured a median of 0.64 s (p90 1.4 s)
  against 77 ms without the stem, more than one link delay per hop, probably because every hop
  opens a substream and negotiates the protocol first.
- Peers that learn the stem graph over many epochs can narrow the source down; the epoch
  length trades this against route stability.

## Implementation notes

Where the code differs from the design above, or adds to it:

- **Channel, not topic.** A stem message carries the broadcast's channel, since the topic is
  inside the ciphertext. A relay checks that the channel matches the message before forwarding.
- **Loops.** A relay that receives a stem message it has already seen publishes it at once. The
  source never publishes its own message: when it comes back, the source sends it on again, at
  most three times, then leaves it to the embargo. The harness picks a fresh hop for that
  re-send; the sharing peer picks one of its two successors (open question 2 in
  `FUTURE_WORK.md` 3.1).
- **Failed hops.** The source re-routes its own message around the failed peer; a relay
  publishes the message instead.
- **Local delivery.** GossipSub does not hand a node its own publishes, so a diffuser that is
  itself a recipient delivers the message to its client directly.
- **Rate limit.** 20 stem messages per second per predecessor (`StemConfig::rate_per_sec`).
- **Fanout.** A diffuser often does not subscribe to the message's channel and publishes
  through GossipSub's fanout. At the pinned rust-libp2p rev the fanout top-up picks
  non-admitted peers, so all nodes with admission scoring set `fanout_ttl` to zero
  (`admission::FANOUT_TTL`).
- **Embargo only inside the channel.** A peer that does not subscribe to the message's channel
  never sees the fluff, so its timer would always fire, and the late copy would reveal it as a
  stem member and, after GossipSub's 60 s duplicate cache, be delivered twice. Such a peer arms
  no embargo. Bitcoin, Monero and Grin do not have this case: every
  node receives every transaction in the fluff, and the mempool rejects late copies. The cost:
  the fail-safe only holds where the source or a later relay subscribes to the channel.
- **Own publishes cancel the embargo.** GossipSub never hands a node its own publication back,
  so a peer that publishes a stem message itself (after a loop or a failed hop) cancels its
  embargo there. Before, the timer fired later and the peer showed the message, and after 60 s
  sent it, a second time.
- **Late stem copies.** A message can reach GossipSub while a copy is still on the stem: a
  relay's embargo runs out early, or a hop arrives although its acknowledgement was lost. Every
  peer remembers which messages it has seen in, or published to, GossipSub, drops stem copies
  of those, and never shows a message twice. Before, a later diffuser showed it again, and a
  later relay armed an embargo that nothing cancelled. Message IDs are kept for two stem-epoch
  lengths, also at peers that never route on the stem.
- **Propagating peer.** A diffuser that delivers a message locally reports its stem
  predecessor to the client, as GossipSub reports the forwarding neighbour; authorship comes
  only from the group signature.
