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
1. Start authorization authority(aa) with `RUST_LOG=info cargo run --package dactie-authority -- -K <folder aa_key_material>`
2. Start archive1 with `RUST_LOG=info cargo run --package dactie-archive --bin dactie-archive -- -K <folder archive_key_material> -u "postgres://archive:archivepw@localhost/archivedb" -i archive1 -a <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`
3. Start archive2 with `RUST_LOG=info cargo run --package dactie-archive --bin dactie-archive -- -K <folder archive_key_material> -u "postgres://archive:archivepw@localhost:5433/archivedb" -i archive2 -a <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`
4. Start peer1 with `RUST_LOG=info cargo run --package dactie-peer --bin dactie-peer -- -K <folder peer1_key_material> -b <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`
5. Start peer2 with `RUST_LOG=info cargo run --package dactie-peer --bin dactie-peer -- -K <folder peer2_key_material> -b <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001`

RUST_LOG=info cargo run --package dactie-peer --bin dactie-peer -- -K ./key_material/key_material_4 -b 12D3KooWLsUHkQfXqqC6DuGaKcZUqYB9bKLWMUgNdnwcRSKdHgBA -B /ip4/127.0.0.1/tcp/10001

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

### Create new Peer with identity of Peer1 (Peer2):
1. On Peer1, request registration tokens: `get_tokens <authorization_peer_id> [count]`. Each new peer uses one; the authority signs at most `--peers-per-member` per member and epoch (`--epoch-secs`).
2. On Peer2, request the opener keys from the authority`get_opener_keys  <authorization_peer_id>`
3. Register a new peer with an existing group key material: `peer_from_sig <authorization_peer_id> <save_file_path>`. It takes a token from the `tokens.json` next to that file.

### Subscribe to Topics:
1. `sub <topic>`

### Exchange a public message:
1. `broad <topic> <msg>`

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

### Trusted archives (Peer):
Members encrypt their key shares to the archives' keys, which the authority hands out. So that a dishonest authority cannot substitute keys it holds, start every peer with each archive's address and PeerId, published out of band: `--archive /ip4/<ip>/tcp/<port>/p2p/<archive PeerId>` (repeat per archive). The peer then confirms every key with its archive before joining, and refuses to join on a mismatch. Without `--archive`, the peer logs a warning and uses the authority's keys unverified.

### Other functions
Other network functions like adding more group members, etc are described in the ReadMe.md in dactie-peer.