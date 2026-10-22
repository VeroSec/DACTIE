# DACTIE: Decentral Anoymous CTI Exchange
This repository contains the artifacts for the implementation and evaluation of DACTIE.
This ReadMe gives you an overview of the Repos Content and a short usage example. More detailed explanations can be found in the subdirectories.

## Folder Structure
- dactie-authority: Implementation of the Authority Peer Type
- dactie-peer: Implementation of the standard Sharing Peer Type
- dactie-archive: Implementation of the Archive Peer Type
- dactie-utils: Helper functions and wrappers, that are used in all peers
- ps16: Implementation of the Pointcheval-Sanders Group Signature
- thressig: Implementation of Short Threshold Dynamic Group Signatures by Carmenisch et al (Modified version for single issuer)
- dactie-experiment-anonymity: The setup of the experiment used to show that gossipsub provides source anonymity
- dactie-experiment-eclipse: The setup of the experiment used to show that eclipse attacks are not possible

## How to use:
Here we explain of how to use our concept based on a simple example. We will have one authority, two archives and two peers.

### Build from Source
This is an explanation of how to build the concept from source. For the Docker Version look below.
1. Install rust(not via Packagemanager)
2. Clone the repos and navigate to the stored folder
3. Build the project by running `cargo build`
4. Create an empty key material folder for each of the 5 peers (e.g. ./key_material/key_material_0..4)
5. Open four terminals and navigate to the dactie folder
6. Start two databases. Either use an own PostgresDB or start the docker container with `docker compose start db db2`

In each terminal start:
1. Start authorization authority(aa) with `RUST_LOG=info cargo run --package dactie-authority -- -K <folder aa_key_material>`. Optional: `--peers-per-member <k>` (default 5) and `--epoch-secs <seconds>` (default 30 days) set how many sharing peers a member can register per epoch, see [Create new Peer with identity of Peer1](#create-new-peer-with-identity-of-peer1-peer2).
2. Start archive1 with `RUST_LOG=info cargo run --package dactie-archive --bin dactie-archive -- -K <folder archive_key_material> -u "postgres://archive:archivepw@localhost/archivedb" -i archive1 -a <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`
3. Start archive2 with `RUST_LOG=info cargo run --package dactie-archive --bin dactie-archive -- -K <folder archive_key_material> -u "postgres://archive:archivepw@localhost:5433/archivedb" -i archive2 -a <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`
4. Start peer1 with `RUST_LOG=info cargo run --package dactie-peer --bin dactie-peer -- -K <folder peer1_key_material> -b <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`
5. Start peer2 with `RUST_LOG=info cargo run --package dactie-peer --bin dactie-peer -- -K <folder peer2_key_material> -b <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`

RUST_LOG=info cargo run --package dactie-peer --bin dactie-peer -- -K ./key_material/key_material_4 -b 12D3KooWLsUHkQfXqqC6DuGaKcZUqYB9bKLWMUgNdnwcRSKdHgBA -B /ip4/127.0.0.1/tcp/10001

Only registered peers take part in GossipSub: a node that has not registered gets no messages, and once a node is registered itself, it closes connections of unregistered nodes after 60 s and keeps at most 5 of them open. Register within a minute of connecting, or the node reconnects when you send the request.

### Local end-to-end test
`scripts/e2e_local.sh` runs the example above automatically and checks every step: registration, sharing peers with slot tags, a second peer of the same member, a broadcast, an unregistered observer, and an opening. It needs Docker (for the two PostgreSQL databases), tmux, and `cargo build --release`, and takes about two minutes. `scripts/e2e_expiry.sh` runs 60-second epochs and checks that a renewed sharing peer stays while one that did not renew is removed and gets no more messages (about three minutes). Logs go to `target/e2e/`; the processes keep running in tmux session `camera` until you run `tmux kill-session -t camera; docker rm -f camera-pg1 camera-pg2`.

### Docker
This section explains the docker setup. If docker is choosen the part above can be ignored
1. Clone the GitRepo and move to the folder
2. Run `build.sh` to create the Docker-Images and create the Key-Material folders
3. Run only the authority node with docker containers with `docker compose up authority_node`. This creates a new peer ID.
4. Copy the BootPeerID into `./key_material/boot_id.env`.  The authority can be stopped after copying the address
5. Start the whole network with `docker compose up`
6. Attach to each container by using `docker attach <container_id>`
7. adminer interface can be found under [https://localhost:8080](https://localhost:8080) with credentials `Username: archive`, `Password: archivepw` and `Database: archivedb`

### End the Archive Initialization Progress(Authority):
`end_init_archive`

This also fixes the opening threshold: how many archives must approve before a signature can be opened. It defaults to a strict majority of the registered archives; start the authority with `--threshold <t>` to choose another value between 1 and the number of archives. Every member splits its key for this threshold, so it cannot change afterwards.

### Register Identity for each peer (Peer1):
1. `register_identity <peer_id_aa> <peer_name>`, peer name can be anything for example peer1 peer2

The join runs from a throwaway identity, so the authority never links this node's own PeerID to the name. Afterwards, the node registers its own PeerID as a sharing peer, which uses one of the member's slots (see below). The authority must be reachable at `-B`, and archives at their `--archive` addresses.

### Create new Peer with identity of Peer1 (Peer2):
1. Request the opener keys from the authority`get_opener_keys  <authorization_peer_id>`. This also fetches the epoch length and how many sharing peers a member may register per epoch (`--peers-per-member`, `--epoch-secs` on the authority).
2. Register a new peer with an existing group key material: `peer_from_sig <authorization_peer_id> <save_file_path>`. It uses one of the member's slots of the current epoch and records it in the `slots.json` next to that file. When all slots are used, the peer reports it.

A sharing peer stays registered until the end of the epoch of its slot. To keep it, run `renew <authorization_peer_id> <save_file_path>` on the sharing peer before the epoch ends; this uses a slot of the next epoch. The authority removes sharing peers that did not renew from the network; a removed peer can register again with `peer_from_sig`.

### Subscribe to Topics:
1. `sub <topic>`

### Exchange a public message:
1. `broad <topic> <msg>`

Messages are anonymous. To have one attributed to your member, e.g. to build reputation, send it with `attr_broad <topic> <msg>` (or `attr_enc_broad <group_number> <msg>` in a private group). Its signature proves the MemberID, and receivers print `Attributed to member: <MemberID>` instead of `Sender: anonymous`.

### Create a group and exchange a private message(all from one Peer):
1. `req_kps <peer_id_bootstrap_peer>`
2. `create_group <peer_id_bootstrap_peer>`
3. `ls_groups` to get group_number
4. `enc_broad <group_number> <msg>`

### Open a message (Authority and Archives):
Opening a signature needs the authority and a threshold of archives: the authority can ask, but each archive operator decides.
1. Send a message over the network to get its signature
2. Ask every archive for its partial opening (authority): `req_open <signature>`
3. Decide on the request in each archive's console (see below): `approve <id>` or `deny <id>`
4. Once enough archives have approved, open the signature (authority): `open <signature>`. It prints the member ID and name, and every PeerID of that member.

#### Archive console
Each archive reads operator commands from its terminal. With Docker, attach with `docker attach <archive container>` (e.g. `archive_node1`) and detach again with `Ctrl+P`, `Ctrl+Q`, which leaves the archive running.

| Command | Effect |
| --- | --- |
| `pending` | Lists opening requests awaiting a decision, with their id and arrival time |
| `approve <id>` | Computes this archive's partial opening and sends it to the authority |
| `deny <id>` | Discards the request |

When the authority runs `req_open`, every archive prints the request's id. A session looks like this:
```
Opening request 3f9c2a1b7e4d5c60 from the authority is pending. Check the evidence, then run `approve 3f9c2a1b7e4d5c60` or `deny 3f9c2a1b7e4d5c60`.
pending
3f9c2a1b7e4d5c60  received 2026-10-11 14:02:51
approve 3f9c2a1b7e4d5c60
Submitted this archive's partial opening for 3f9c2a1b7e4d5c60
```
Pending requests are stored in the archive's database (table `pending_opens`), so they survive a restart and the operator can decide later.

For automated tests only, start an archive with `--auto-approve-open`: it then answers every request without approval, so the authority could open any signature on its own.

### Connection watermarks (Peer):
Each sharing peer keeps between 50 and 100 connections: it never holds more than `--conn-high` (default 100), and once full it closes connections down to `--conn-low` (default 50), sparing its gossipsub mesh peers, the authority, trusted archives and connections younger than 20 s. This follows go-libp2p's connection manager and keeps large networks from connecting every peer to every other. A full peer refuses new connections before the Noise handshake, like go-libp2p's resource manager, instead of paying for a handshake it then rejects. Inbound connections may fill at most 80% of the slots, so a peer whose inbound side is saturated can still dial, and at most 32 inbound handshakes may be pending at once. The authority and trusted archives are exempt whenever the peer knows their PeerId, i.e. when it dials them. The authority and archives themselves are not capped, since every peer has to reach them.

### Stem phase (Peer):
A peer's own anonymous messages (broadcasts and group messages) first travel along a random stem of sharing peers (`/dactie/stem/1`), and only the peer at its end publishes them into GossipSub, so a colluding mesh neighbour cannot tell the source from the relays (Dandelion++). `--stem-fluff-probability` (default 0.2) is the chance that a peer ends stems in an epoch; `0` turns the stem off. See the Stem phase section of `docs/PROTOCOL.md`.

### Trusted archives (Peer):
Members encrypt their key shares to the archives' keys, which the authority hands out. So that a dishonest authority cannot substitute keys it holds, start every peer with each archive's address and PeerId, published out of band: `--archive /ip4/<ip>/tcp/<port>/p2p/<archive PeerId>` (repeat per archive). The peer then confirms every key with its archive before joining, and refuses to join on a mismatch. Without `--archive`, the peer logs a warning and uses the authority's keys unverified.

### Other functions
Other network functions like adding more group members, etc are described in the ReadMe.md in dactie-peer.