# DACTIE Protocol

This document describes how the DACTIE prototype works at the protocol level: who sends which message to whom, what is checked, and what is stored where. The paper presents the design and its evaluation. This document covers the protocol detail the paper leaves out. For build and usage instructions, see the [README](../README.md).

All statements describe the code in this repository. Message names refer to `dactie-utils/src/shared_structs.rs`.

## Roles

| Role | Crate | Purpose |
| --- | --- | --- |
| Authorization Authority (AA) | `dactie-authority` | Admits members and archives, fixes the opening threshold `t`, owns the common MLS group, and coordinates signature openings. It also joins as member `0` ("Authority"). |
| Archive (ARC) | `dactie-archive` | Stores every message it receives, holds one ElGamal key pair, and stores one encrypted Shamir share of every member key. After its operator approves, it contributes a partial opening. An archive can verify group signatures but cannot create them. |
| Sharing peer | `dactie-peer` | Publishes and receives CTI. A member can run up to `k` sharing peers per epoch, each with its own PeerID, none of which the AA can link to the member. The node a member joins from becomes its first sharing peer; the join itself runs from a throwaway identity (section 2). |

## Identifiers and keys

| Name | What it is | Held by |
| --- | --- | --- |
| PeerID | libp2p peer ID, derived from the node's Ed25519 key in its key-material folder | each node |
| Throwaway join identity | Fresh libp2p key a peer joins from; the only PeerID the AA links to a name, used once and dropped (`dactie-peer/src/join_relay.rs`) | joining peer, briefly |
| MemberID | Numeric ID derived by hashing the name a member registers with | AA (`gml`: MemberID → name) |
| Group public key / manager key | Keys of the threshold dynamic group signature scheme (`thressig`, Camenisch et al. 2020, single-issuer variant) | AA (manager key); all nodes (group key) |
| Member key | Member secret `sk` plus the AA's credential `(σ1, σ2, e1)` on it, with `σ1 = g^{H0(MemberID)}`; signing randomizes the credential unless the message is attributed (section 4) | member peers (`key_material.json`) |
| Slot tag | `H(epoch, slot)^sk` for one of a member's `k` slots per epoch; registers or renews one sharing peer, unlinkable to the member (section 3) | AA (used tags of the current epochs); member (`slots.json` next to `key_material.json`) |
| Opener key | ElGamal key pair in G2, one per archive | archive (secret key); AA and peers (public key, with the archive's PeerID) |
| MLS credential | MLS basic credential holding the PeerID bytes | each node |

## Running the nodes

Each node is one binary. It takes its options on the command line and reads console commands from stdin (with Docker: `docker attach <container>`, detach with Ctrl-P Ctrl-Q). `RUST_LOG=info` shows the log lines this document refers to. Multiaddrs have the form `/ip4/<address>/tcp/<port>`.

### dactie-authority

```text
dactie-authority -K <dir> [-p <port>] [--advertise-addr <multiaddr>]
                 [--threshold <t>] [--epoch-secs <seconds>] [--peers-per-member <k>]
```

| Option | Default | What it does |
| --- | --- | --- |
| `-K, --km-dir <dir>` | required | Folder of the AA's state: `key_material.json` (its libp2p key and the group manager key) and `storage.json` (members, archives, threshold, used slot tags, registered sharing peers). The AA keeps its PeerID only if both files exist; delete them together to start a new network. |
| `-p, --port <port>` | `10001` | TCP port the AA listens on, on all interfaces. Archives and peers reach it at `-B /ip4/<host>/tcp/<port>`. |
| `--advertise-addr <multiaddr>` | none | Address the AA announces to others, e.g. `/ip4/10.244.192.116/tcp/10001`, for hosts with several interfaces or behind NAT. Ignored for `127.0.0.1`. |
| `--threshold <t>` | strict majority, `⌊n/2⌋ + 1` | How many of the `n` archives must approve before a signature can be opened. It takes effect when `end_init_archive` closes the init phase, and is fixed from then on, because every member shares its key for it; a different value on a later start is ignored with a warning. Allowed: `1 ≤ t ≤ n`. The AA warns at `t ≤ n/2` (a minority of archives can open signatures) and at `t = n` (one unavailable archive blocks every opening). |
| `--epoch-secs <seconds>` | `2592000` (30 days) | Epoch length. A sharing peer's registration lasts until the end of its slot tag's epoch; epochs start at multiples of this length in Unix time. Shorter epochs remove unused sharing peers sooner, but members must renew more often. |
| `--peers-per-member <k>` | `5` | Slots per member and epoch: the most sharing peers a member can have registered at once, including the node it joined from. `m` colluding members run at most `m·k` sharing peers. |
| `-l, --load-file <file>` | | Not used. |

Console: `end_init_archive` closes the init phase (section 1); `req_open <signature hex>` and `open <signature hex>` open a signature (section 7).

### dactie-archive

```text
dactie-archive -K <dir> -u <postgres-url> -a <AA PeerID> -B <AA multiaddr> -i <name>
               [-p <port>] [--advertise-addr <multiaddr>] [--auto-approve-open]
```

| Option | Default | What it does |
| --- | --- | --- |
| `-K, --km-dir <dir>` | required | Folder of the archive's `key_material.json`: its libp2p key, its ElGamal opening key, and the group key. The archive registers with the AA whenever this file holds no group key yet. |
| `-u, --url <url>` | required | PostgreSQL URL of this archive's own database, e.g. `postgres://archive:archivepw@127.0.0.1:5432/archivedb`. The tables are in `init.sql`. The archive exits at startup if the database is unreachable. |
| `-a, --aa-id <PeerID>` | required | The AA's PeerID, as in the AA's log line `Local PeerId`. |
| `-B, --aa-addr <multiaddr>` | `/ip4/127.0.0.1/tcp/20011` | The AA's address. Set it explicitly: the default port is not the AA's default `10001`. |
| `-i, --identity <name>` | required | The name this archive registers with at the AA, e.g. `archive1`. Free text, unrelated to Docker profiles or container names. The AA derives the archive's ID from it and refuses empty names and names it has already registered, so every archive needs its own. It is used only for registration and shows in the AA's log as `Verified identity <name> from <PeerID>`. |
| `-p, --port <port>` | random | TCP port the archive listens on. |
| `--advertise-addr <multiaddr>` | none | As for the AA. |
| `--auto-approve-open` | off | Answers every opening request without asking the operator. For automated tests only: with it, the AA alone could open any signature. |

Console: `pending`, `approve <id>`, `deny <id>` (section 7).

### dactie-peer

```text
dactie-peer -K <dir> -b <AA PeerID> -B <AA multiaddr> [-p <port>] [--advertise-addr <multiaddr>]
            [--archive <multiaddr>/p2p/<PeerID>]... [--conn-low <n>] [--conn-high <n>]
            [--stem-fluff-probability <q>] [--stem-epoch-secs <s>] [--stem-embargo-secs <s>]
            [--metrics-addr <ip:port>]
```

| Option | Default | What it does |
| --- | --- | --- |
| `-K, --km-dir <dir>` | required | Folder of the peer's state: `key_material.json` (libp2p key and member key), `slots.json` (the slots its member has used) and `group_saves/` (MLS groups and subscribed topics, restored on restart). Other sharing peers of the same member point `peer_from_sig` and `renew` at this `key_material.json`. |
| `-b, --authority-peer-id <PeerID>` | required | The AA's PeerID. |
| `-B, --bootstrap-addr <multiaddr>` | `/ip4/127.0.0.1/tcp/5005` | The AA's address. The peer finds other peers through it, and a join's throwaway identity connects to it. Set it explicitly: the default port is not the AA's default `10001`. |
| `-p, --port <port>` | random | TCP port the peer listens on. |
| `--advertise-addr <multiaddr>` | none | As for the AA. |
| `--archive <multiaddr>/p2p/<PeerID>` | none | A trusted archive, configured out of band; repeat for each archive. Before the peer uses the archive keys the AA hands out (join, `get_opener_keys`), it asks each archive for its own key and refuses on a mismatch, so a dishonest AA cannot substitute keys it holds. Without it, the peer logs a warning and uses the AA's keys unverified. |
| `--conn-high <n>` | `100` | The most connections the peer holds; further ones are refused before the handshake. Incoming connections take at most 80% of them, so the peer can always still dial out, and at most 32 incoming handshakes may be in progress. |
| `--conn-low <n>` | `50` | Once the peer reaches `--conn-high`, it closes random connections older than 20 s down to this number, never its GossipSub mesh peers, the AA or trusted archives. |
| `--stem-fluff-probability <q>` | `0.2` | Stem phase (see [Stem phase](#stem-phase-dandelion)): probability that the peer is a diffuser in an epoch. `0` turns the stem off; anonymous broadcasts are then published directly. |
| `--stem-epoch-secs <s>` | `600` | How long the peer keeps its two stem successors and its role (plus up to 30 s of jitter). |
| `--stem-embargo-secs <s>` | `39` | Mean of the exponential fail-safe after which a peer that sent a message on the stem publishes it itself if it has not shown up in GossipSub. |
| `--metrics-addr <ip:port>` | off | Serves Prometheus metrics, e.g. `127.0.0.1:9898`. The endpoint is unauthenticated: bind it to loopback unless the network is trusted. |
| `-n, --n-memkey <n>`, `-l, --load-file <file>` | | Not used. |

Console: `register_identity` (section 2); `get_opener_keys`, `peer_from_sig`, `renew` (section 3); `sub`, `unsub`, `broad`, `attr_broad`, `enc_broad`, `attr_enc_broad` (section 4); `req_kps`, `create_group`, `ls_groups`, `add_group_member`, `remove_group_member`, `delete_group_for_all`, `update_key_material`, `update_groups` (sections 5 and 6); `peer_id`. A peer prints the list on any unknown command.

## Network layer

All nodes use libp2p over TCP with Noise and Yamux. GossipSub is configured as follows (`mybehaviour.rs` of each crate):

- `validate_messages()`: the application checks every message before GossipSub forwards it.
- Maximum message size 8 MiB, instead of the default 64 KiB that silently dropped complete MISP events.
- Sharing peers use `flood_publish(false)` and `MessageAuthenticity::Anonymous`. Without flood publishing, a new message leaves its publisher only towards its mesh peers. Anonymous authenticity puts no author or sequence number into gossip messages.
- All nodes use the mesh band `mesh_n_low = 10`, `mesh_n = 12`, `mesh_n_high = 24` (`dactie-utils/src/net.rs`). GossipSub grafts only below `mesh_n_low` and prunes only above `mesh_n_high`, so the paper's anonymity bound is evaluated at `mesh_n_high`. The experiments in `dactie-experiment-anonymity` and `dactie-experiment-eclipse` set the band themselves with `-D`, `--mesh-n-low` and `--mesh-n-high`.

### Admission to GossipSub

Any libp2p node can connect to a DACTIE node, e.g. to register with the AA or to fetch archive keys, and GossipSub itself has no access control. Every node therefore scores its GossipSub peers by admission (`dactie-utils/src/admission.rs`): PeerIDs in the common MLS group, i.e. the AA, the archives and every registered sharing peer, score 0; all others score −60, below GossipSub's publish threshold. A node never grafts a peer with a negative score into a mesh, prunes such peers at every heartbeat, and sends them neither its own or forwarded messages nor gossip. Subscriptions of non-admitted peers are still recorded, so a new peer joins meshes as soon as the commit that admits it arrives. Like members, non-admitted peers still see which channels a node subscribes to. Nodes re-score their peers whenever the common group changes. A node publishing to a channel it does not subscribe to (the AA, or a stem diffuser) uses GossipSub's fanout; at the pinned rust-libp2p rev the heartbeat tops a fanout up with peers *below* the publish threshold, so every node sets the fanout TTL to zero and each publish picks fresh fanout peers (`admission::FANOUT_TTL`).

Non-admitted nodes would still occupy connection slots, and PeerIDs cost nothing, so an unregistered node could fill every slot and cut a node off from members. Once a node is admitted itself, it keeps at most 5 non-admitted connections open at once, refuses further ones, and closes each after 60 s, enough to register with the AA or fetch archive keys (`admission::Gate`). Before a node is admitted, the gate does nothing, so a newcomer can reach the AA and the archives.

### Partitioned channels

A topic is not a GossipSub topic of its own. `get_channel` in `dactie-utils/src/mls_wrapper.rs` maps it to one of `NUM_CHANNELS = 10` channels:

```text
channel = "channel_" + (hash(topic) mod 10)      // topics "0".."9" map directly to their channel
```

A peer subscribes to a channel in GossipSub when it subscribes to the first topic in that channel. It receives and decrypts every broadcast on the channel, but delivers only those whose topic it subscribed to. Private MLS groups use the channel of their group ID. Archives subscribe to all channels.

### Gossip messages and validation

A gossip message is the bincode encoding of `Message { body: MessageBody, signature }`, where `signature` is a group signature over the encoded body. A receiver:

1. decodes the message;
2. verifies the group signature;
3. reports `MessageAcceptance::Accept` if the signature is valid, so GossipSub forwards the message. This happens before decryption, so peers also forward messages they cannot read;
4. reports `MessageAcceptance::Reject` if the signature is invalid, so the message is dropped and not forwarded.

Direct requests (libp2p request-response) carry an optional group signature. Which requests must be signed is noted in each flow below.

### Stem phase (Dandelion++)

Without it, a sharing peer hands its broadcast to all its mesh peers, and a colluder holding one of those mesh slots names the source with first-spy. With the stem, a peer's own anonymous broadcasts (`broad`, `enc_broad`) first travel along a random path of sharing peers, one hop at a time, and only the peer at its end publishes them into GossipSub. Attributed broadcasts and MLS control messages are published directly.

- **Protocol:** `/dactie/stem/1` (libp2p request-response, length-prefixed bincode). The request is `StemMessage { channel, payload }`: the signed gossip message exactly as it would be published, and its channel. The answer is an empty acknowledgement.
- **Epochs:** every `--stem-epoch-secs` (plus up to 30 s of jitter) a peer picks two successors among its connected, admitted peers whose identify info lists the protocol (so never the AA or an archive), and becomes a diffuser with probability `--stem-fluff-probability`. For the whole epoch, each predecessor is mapped to one successor, and the peer's own messages always go to the same successor, so repeated messages take the same path.
- **Relays:** a peer accepts stem messages only from admitted peers, at most 20 per predecessor and second. It forwards only a `BroadcastEnc` or `GroupEnc` with a valid group signature whose channel matches the request. A diffuser publishes it; any other peer passes it to its mapped successor.
- **Source:** a peer's own messages enter the stem even if it is a diffuser. It never publishes them itself: if the stem loops back to it or its hop fails, it re-routes the message to a peer that does not have it yet (its other successor, else a spare connection), up to three times, and otherwise leaves it to the embargo. It sends a message to the same peer twice only if every connected stem peer has it already, and then back to its first hop: only a source ever sends a message twice, so that peer learns its predecessor is the source, but as the first hop it got the message straight from the source anyway. It does not show its own message again when it comes back through GossipSub.
- **Fail-safe:** a peer that sent a message on the stem and subscribes to its channel publishes it itself if it has not arrived through GossipSub when an exponential embargo (mean `--stem-embargo-secs`) runs out. A peer outside the channel never sees the message in GossipSub and arms no embargo: its timer would always run out, and the late copy would mark it as a stem member and, after GossipSub's 60 s duplicate cache, be delivered a second time. Relays also publish when their hop fails or the stem loops back.
- **Authorship:** the stem carries the source's signed message unchanged, and every hop checks the group signature, so an opening still names the real author, never a relay. Peers show the sender as the signature states it (anonymous, or the attributed member). A peer that ends a stem and is itself a recipient shows its stem predecessor as the propagating peer, as it shows the forwarding neighbour for gossip.
- **Limits:** the first stem hop sees the source's PeerID; if it colludes, first-spy succeeds. In the anonymity harness, first-spy success fell from 62% to 10% with 20 of 200 colluders (about the colluding share) at q = 0.2, at about 0.6 s more median latency (docs/DANDELION.md).

## 1. Archive initialization

Archives must register before any member joins, because every member splits its key into one share per archive.

1. Start the AA (options: [dactie-authority](#dactie-authority)). Choose `--threshold` now: `end_init_archive` fixes it below.
2. Each archive starts with its own name, `-i <name>` (any unique text such as `archive1`; see [dactie-archive](#dactie-archive)). If its key file holds no group key, it sends an unsigned `VerifyIdentity { identity: name, with_signature: false }` to the AA.
3. The AA derives the MemberID from the name and rejects empty or already registered names. It replies `IdentityVerified { id, group_key }`.
4. The archive generates its ElGamal key pair and an MLS KeyPackage, and sends `JoinKeyPackage { keypackage, elgamal_pk }`.
5. The AA records the archive's PeerID and ElGamal key, keeps the KeyPackage, and replies `MemKey { None }`. Archives therefore receive no member key. The position of an archive in this list is the index of the share it will hold.
6. The operator runs `end_init_archive` on the AA:
   - At least two archives must be registered.
   - `t` is set to `--threshold` or, by default, a strict majority (`⌊n/2⌋ + 1`), with `1 ≤ t ≤ n`. The value is stored and cannot change afterwards, because every member key is shared for it.
   - The AA joins as member `0`.
   - The AA creates the common MLS group with all archives, becomes its owner, and sends each archive a group-signed `JoinGroup { welcome, ratchet_tree }`. Archives accept `JoinGroup` for the common group only from the AA.
   - The AA sends each archive its share of the AA's own member key in `AddIDShare`.

The AA keeps the archives' KeyPackages in memory only, so do not restart it between archive registration and `end_init_archive`.

## 2. Member registration

Console: `register_identity <AA PeerID> <name>`.

The AA sees the member's name together with the PeerID a join comes from. So that it cannot link the node's own PeerID to the member, the peer sends the messages of the join from a **throwaway identity** (`dactie-peer/src/join_relay.rs`): a fresh libp2p key, used only for this join and dropped once the member key has arrived. The throwaway identity dials the AA at `-B` and the archives at their `--archive` addresses. Afterwards, the node registers its own PeerID as a sharing peer like any other (section 3), which does not reveal the member.

```mermaid
sequenceDiagram
    participant T as Peer (throwaway identity)
    participant P as Peer (own PeerID)
    participant AA as Authority
    participant A as Archive i
    T->>AA: VerifyIdentity {name}
    AA->>T: IdentityVerified {id, n, group key, (archive PeerID, opener key)*, t, epoch length, k}
    opt peer started with --archive
        T->>A: RequestOpenerKeys
        A->>T: ArchiveOpenerKey {elgamal_pk}
    end
    T->>AA: JoinKeyMaterial {g^sk, h^sk, π1, π2, π3, encrypted shares, commitments}
    AA->>A: AddIDShare {id, encrypted share i, commitments} (group-signed)
    A->>A: verify share, store in id_shares
    AA->>T: MemKey {σ1, σ2, e1} (group-signed)
    Note over T: throwaway identity dropped
    P->>AA: AddPeerToSig with slot 1 (section 3)
```

1. **Identity.** The peer sends `VerifyIdentity { name, with_signature: true }`. In a deployment, this is where the AA would vet the organization out of band. The prototype accepts any new name. The AA derives the MemberID from the name, draws a random nonce `n`, and remembers `(n, MemberID, name)` under the requesting (throwaway) PeerID until the join completes.
2. **Join parameters.** The AA replies `IdentityVerified` with the MemberID, `n`, the group public key, every archive's opener key paired with its PeerID, `t`, the epoch length and `k`.
3. **Archive key check.** If the peer was started with `--archive <multiaddr>/p2p/<PeerID>` (repeatable), it asks each listed archive for its key directly (`RequestOpenerKeys` → `ArchiveOpenerKey`), also from the throwaway identity, and compares it with the key the AA sent. On a mismatch, the peer abandons the join. This keeps a dishonest AA from substituting archive keys it controls. Without `--archive`, the peer logs a warning and trusts the AA's keys.
4. **Key material** (`thressig::join_mem1`, `create_enc_shares`). The peer
   - chooses its secret `sk` and computes `g^sk` and `h^sk` with `h = g^{H0(MemberID)}`;
   - proves knowledge of both discrete logarithms, bound to `n` (`π1`, `π2`);
   - blinds `h` with a random `α` and Shamir-shares `sk` with a polynomial of degree `t − 1`, publishing `t` Feldman commitments under the blinded base;
   - encrypts share `i` with archive `i`'s ElGamal key and attaches a proof that the ciphertext matches the commitments;
   - proves that the first commitment hides the same `sk` as `h^sk` (`π3`, equality of discrete logarithms, bound to `n`).

   It sends all of this in `JoinKeyMaterial`. `sk` never leaves the peer.
5. **AA checks** (`thressig::join_mgr2`). The AA checks:
   - the number of commitments equals `t`, and the number of shares equals the number of archives;
   - the MemberID is new;
   - `π1` and `π2` against `n`;
   - that all shares use one base and share `i` has index `i + 1`, so any `t` archives can interpolate;
   - `π3`, so the shares hide the `sk` the credential is issued on. Otherwise a member could share another secret and never be opened;
   - every encrypted share against its archive's key and the commitments.

   It then issues the credential `(σ1, σ2, e1)` on `sk` without learning `sk`, and records `MemberID → name` in `gml`.
6. **Shares to archives.** The AA sends archive `i` a group-signed `AddIDShare { id, enc_share_i, commitments }`. The message carries no PeerID. The archive verifies the AA's signature and the share against its own key, then stores it in `id_shares`.
7. **Member key.** The AA sends `MemKey { σ1, σ2, e1 }`. It does not add the joining PeerID to the common group. The peer combines the credential with `sk` and can now sign, drops the throwaway identity, and registers its own PeerID as a sharing peer with slot 1 of the current epoch.

**What the AA learns.** The AA learns the member's name and MemberID, and the throwaway PeerID they came from, which is never used again. After the join, the AA keeps `gml` (MemberID → name) and the list of admitted sharing PeerIDs separately; no sharing PeerID is linked to a name. It never sees `sk` or any share in the clear. The AA still sees the IP address of the join; joining from another network location, or through Tor, hides that too. It could also link a join and a sharing peer that registers right after it by timing.

## 3. Additional sharing peers

A member can run further sharing peers under the same member key. Each new peer has its own PeerID and MLS credential, and uses one of the member's `k` slots of the current epoch, so that `m` colluding members run at most `m·k` sharing peers at a time.

**Slot tags** (`thressig/src/tags.rs`). The AA is started with `--epoch-secs` (default 30 days) and `--peers-per-member` `k` (default 5); the epoch is the Unix time divided by the epoch length. A member has slots `1..=k` per epoch, and its tag for a slot is `T = H(epoch, slot)^sk`. The same member always gets the same tag for the same epoch and slot, so the AA accepts each tag once, while tags of different members, slots or epochs cannot be linked. A peer proves its tag without revealing the member (`thressig::prove_member`: a freshly randomized credential and one Schnorr proof for the credential equation and every statement about `sk`). The member therefore never contacts the AA in a way that identifies it after the join, and the AA does not learn which member registers a peer or how many peers a member runs. Which slots a member has used is recorded in `slots.json` next to its `key_material.json`.

1. `get_opener_keys <AA PeerID>`: the new peer fetches the archives' keys, `t`, the epoch length and `k` (`RequestOpenerKeys` → group-signed `OpenerKeys`). If the peer was started with `--archive`, it confirms the keys as in step 3 above.
2. `peer_from_sig <AA PeerID> <key_material.json of an existing peer>`: the new peer loads the member key, claims the lowest free slot of the current epoch in the `slots.json` next to that file, shares the same `sk` again with fresh blinding and a fresh polynomial, and sends a **group-signed** `AddPeerToSig { keypackage, enc_shares, commitments, tag, proof }`. Because the request is group-signed, the AA learns only that the requester is some admitted member, not which one. `proof` shows that the first commitment and the slot tag hide the `sk` of a valid member credential, bound to the KeyPackage. It reveals no more about the member than a group signature.
3. The AA checks `proof` and the share layout, the KeyPackage credential against the requesting PeerID, the share count, and every encrypted share. Last, it accepts the tag if it is for the current epoch and a slot in `1..=k` and was not used before, and records it. It then sends archive `i` a group-signed `AddPeerShare { peer_id, enc_share_i, commitments }`, adds the peer to the common group, and replies `PeerAdded`; otherwise it replies `PeerRefused { reason }`.
4. Each archive verifies the share and stores it in `peer_shares` under the new PeerID.

Archives store `id_shares` per MemberID without any PeerID, and `peer_shares` per additional PeerID without any MemberID. Only an opening links the two.

### Renewal and expiry

A sharing peer's registration lasts until the end of its tag's epoch.

- **Renewal.** Before the epoch ends, the sharing peer runs `renew <AA PeerID> <key_material.json of the member>`: it claims a slot of the next epoch in that `slots.json` and sends the unsigned `RenewPeer { tag, proof }`, with the proof bound to its own PeerID. The AA accepts it only from registered sharing peers and only for a tag of the current or the next epoch it has not seen, and replies `PeerRenewed { until_epoch }`.
- **Expiry.** Every 5 s, the AA removes sharing peers whose registration ended with an earlier epoch from the common group with a commit. The admission check then excludes them from GossipSub on every node. A removed peer can register again with `peer_from_sig` and a slot of the current epoch.
- **Openings.** The archives keep the `peer_shares` of removed peers, and the AA keeps their PeerIDs in `allowed_peers`, so their old messages can still be opened to them.
- **Cap across epochs.** A new sharing peer needs a slot of the current epoch, and a renewal uses one of the next epoch's `k` slots. A member therefore never has more than `k` sharing peers registered for any epoch, also at the epoch boundary.

## 4. Publishing and receiving CTI

| Console | Gossip message | Encryption | Readable by |
| --- | --- | --- | --- |
| `sub <topic>` | none (subscribes to the topic's channel) | — | — |
| `broad <topic> <msg>` | `BroadcastEnc { channel, data, nonce }`, where `data` encrypts `BroadcastPlain { topic, data }` | AES-256-GCM with a random 96-bit nonce, under a key exported from the common MLS group's current epoch | all members (the common group) |
| `enc_broad <group> <msg>` | `GroupEnc { group_id, enc_data, nonce }` | AES-256-GCM under a key exported from the private group's current epoch | members of that private group |
| `attr_broad <topic> <msg>`, `attr_enc_broad <group> <msg>` | as above, with `attributed_to: MemberID` | as above | as above; receivers also see the sender's MemberID (see Attribution) |

Only the channel of a broadcast travels unencrypted; its topic is inside the ciphertext, so nobody without the common-group key learns what a message is about, and an archive queried by channel learns only the channel. The group ID of a private-group message travels unencrypted. The exported keys change with every MLS epoch. The sender signs the whole encoded body (channel or group ID, ciphertext, nonce) with its group signature and publishes it on the channel of the topic or group.

A receiver first rejects a `BroadcastEnc` whose signed channel differs from the GossipSub topic it arrived on, so a broadcast cannot be replayed on another channel, and then validates the signature (see [Gossip messages and validation](#gossip-messages-and-validation)). It decrypts every `BroadcastEnc`, drops it if the topic inside does not belong to the channel, and delivers it only if it subscribed to that topic. It decrypts a `GroupEnc` only if it is a member of the group. It ignores all other messages, but still forwards them.

Messages are anonymous: neither the gossip message nor the group signature identifies the sender.

**Attribution.** A sender who wants a message attributed to it, e.g. to build reputation, uses `attr_broad <topic> <msg>` or `attr_enc_broad <group> <msg>`. The peer then signs `(body, MemberID)` without randomizing its credential (`thressig::sign_attributed`): `σ1` stays `h = g^{H0(MemberID)}`, which anyone can compute from the MemberID, and only the member holding that credential can sign with it. The gossip message carries the MemberID in `attributed_to`. Receivers verify the signature over `(body, MemberID)` and check `σ1 = h` (`thressig::signed_by`), so the MemberID can be neither forged nor removed, and print `Attributed to member: <MemberID>`; anonymous messages print `Sender: anonymous`. An attributed signature reveals the member's credential `(σ1, σ2)`; its anonymous signatures stay unlinkable to it under XDH, the assumption their anonymity already rests on. Archives store the message without the attribution.

## 5. MLS groups

- `req_kps <PeerIDs>`: collect KeyPackages from the peers to be added. Requests and answers are group-signed.
- `create_group <PeerIDs>`: the creator becomes the group's **owner** (leaf 0). It adds the peers with a commit, publishes the commit on the group's channel, and sends the new members a group-signed `JoinGroup` welcome.
- `add_group_member`, `remove_group_member`, `update_key_material`: any member publishes these as **proposals**.
- **Only the owner commits.** When the owner receives a proposal, it commits all pending proposals and publishes the commit after a short delay. Receivers reject commits from any other member. In an unordered peer-to-peer network, concurrent commits from different members would otherwise fork the group. Proposals to remove the owner are ignored. If a non-owner proposed an Add, it sends the welcome once it has processed the owner's commit.
- The **common group** is owned by the AA. The AA adds every member and every additional peer by direct commit. Peers cannot address the common group from the console.

## 6. Archive storage

Each archive writes to its own PostgreSQL database (`init.sql`):

| Table | Content | Form |
| --- | --- | --- |
| `broadcast(id, channel, topic, data)` | common-group messages, indexed by channel | **decrypted** (archives are members of the common group) |
| `group_enc(id, group_id, data, nonce)` | private-group messages | ciphertext (archives are not members of private groups) |
| `proposal`, `commit` | MLS proposals, commits and welcomes, with group ID and epoch | as received |
| `id_shares(id, enc_share, commits)` | one encrypted share per MemberID | ElGamal ciphertext |
| `peer_shares(peer_id, enc_share, commits)` | one encrypted share per additional PeerID | ElGamal ciphertext |
| `pending_opens(id, signature, received_at)` | opening requests awaiting the operator | — |

`id` is the GossipSub message ID, a hash of the message content. Data is kept indefinitely.

**Catching up.** A peer that was offline sends a group-signed `Update { group_id, epoch }` to an archive for each private group. The archive returns all stored commits after that epoch and all proposals from that epoch on, and the peer applies them in order. The archive ignores `Update` requests without a valid group signature. Stored CTI messages are not served over the network: they can be searched in the archive's database, e.g. through the Adminer interface of the Docker setup.

## 7. Opening a signature

Opening reveals the member behind a group signature. It needs the AA and at least `t` archives whose operators agree. The AA can ask, but each archive operator decides.

1. **Request.** The AA operator runs `req_open <signature hex>`. The AA sends a group-signed `RequestOpen { signature }` to every archive. Archives act on opening requests only if they come from the AA.
2. **Approval.** Each archive stores the request in `pending_opens`, prints its ID, and replies `OpenPending`. Its operator reviews the evidence and runs `approve <id>` or `deny <id>` (`pending` lists open requests). `--auto-approve-open` skips this step and is meant for automated tests only.
3. **Partial opening.** On `approve`, the archive decrypts the pairing form of each of its stored shares. The signer is unknown, so it does this for **every** row in `id_shares` and `peer_shares`, computing `e(σ1, Ỹ1^{s_i})` with its ElGamal secret key. It sends the results to the AA in `SubmitOpenPairings`. The AA accepts these only from registered archives. The cost grows linearly with the number of members, which keeps openings deliberate and rare.
4. **Combination.** Once partial openings from at least `t` distinct share indices are in, the AA operator runs `open <signature hex>`. For every MemberID in `gml`, the AA combines the archives' pairings with Lagrange coefficients and checks them against the signature (`thressig::open`). It repeats this for every admitted PeerID using `peer_shares`.
5. **Result.** The AA prints the matching MemberID and name, and every additional PeerID of that member. It then deletes the stored pairings for this signature.

Sharing peers cannot request openings. A peer that observes misbehavior reports the signature, which the receiving console prints as hex, to the AA out of band.

## Relation to the Camenisch et al. scheme

DACTIE uses the Short Threshold Dynamic Group Signatures of Camenisch, Drijvers, Lehmann, Neven and Towa (2020), with one issuer, the AA, instead of a distributed issuer and a public ledger. The member list (`gml`) lives at the AA, and the per-member opening shares live in the archives' databases. Joining follows the scheme's `GJoin` protocol: the member proves knowledge of `sk`, Shamir-shares it to the openers under ElGamal encryption, and receives a credential on `sk`. The prototype adds two steps: the member may first confirm the openers' keys with the openers themselves, and a member adding a sharing peer proves with `share_link` that the new shares hide its key, since the issuer cannot see which member asks. Signing and verification are unchanged.
