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

### Register Identity for each peer (Peer1):
1. `register_identity <peer_id_aa> <peer_name>`, peer name can be anything for example peer1 peer2

### Create new Peer with identity of Peer1 (Peer2):
1. Request the opener keys from the authority`get_opener_keys  <authorization_peer_id>`
2. Register a new peer with an existing group key material: `peer_from_sig <authorization_peer_id> <save_file_path>`

### Subscribe to Topics:
1. `sub <topic>`

### Exchange a public message:
1. `broad <topic> <msg>`

### Create a group and exchange a private message(all from one Peer):
1. `req_kps <peer_id_bootstrap_peer>`
2. `create_group <peer_id_bootstrap_peer>`
3. `ls_groups` to get group_number
4. `enc_broad <group_number> <msg>`

### Open a message(Authority):
1. Send a message over the network to get its signature
2. Get the opening pairings from the archives with: `req_open <signatur>`
3. Open the signature: `open <signature>`

### Other functions
Other network functions like adding more group members, etc are described in the ReadMe.md in dactie-peer.