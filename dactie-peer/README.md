# DACTIE-Peer
This package contains the implementation of the sharing peer. 


## Startup
```
Usage: dactie-peer [OPTIONS] --km-dir <KM_DIR>
Example: dactie-peer -K <folder peer1_key_material> -b <peer_id_aa> -B /ip4/127.0.0.1/tcp/10001

Options:
  -b, --bootstrap-peer-id <BOOTSTRAP_PEER_ID>
          PeerID of a Bootsrap Node, if none is given, the peer starts as Bootstrap Node on Port 10001
  -B, --bootstrap-addr <BOOTSTRAP_ADDR>
          PeerID of a Bootsrap Node, if none is given, the peer starts as Bootstrap Node on Port 10001 [default: /ip4/127.0.0.1/tcp/5005]
  -K, --km-dir <KM_DIR>
          Location of the folder, where the keymaterial should be saved
  -n, --n-memkey <N_MEMKEY>
          Number of Memberkeys to genereate [default: 1]
  -p, --port <PORT>
          Port to run service on
  -l, --load-file <LOAD_FILE>
          Load_file, for stored groups and subscribed topics
  -h, --help
          Print help
  -V, --version
          Print version
```

## Commands

List of available commands. It can be opened in the peer by typing help.
```
- ls_groups: Lists all available groups and prints the group_number + group ids
- req_kps <peer_id_1>, <peer_id_2>, ...: Requests kps from the peers in the arguments
- create_group <peer_id_1>, <peer_id_2>, ...: Creates a group with the mentioned peers
- enc_broad <group_number> <msg>: Sends an encrypted and signed message to the group
- sub <topic>: Subscribes to a topic
- unsub <topic>: Unsubscribes from a topic
- broad <topic> <msg>: Sends a public available message to the group
- add_group_member <group_number> <peer_id>: Adds a new Group Member
- remove_group_member <group_number> <peer_id>: Removes peer from group, except group owner,
- delete_group_for_all <group_number>: Deletes Group for all peers, only callable by owner
- update_key_material <group_number>: Updates own key material in group
- update_groups <archive_peer_id>: Requests updates for all groups from an archive
- peer_id: Prints out PeerId
- register_identity <authorization_peer_id> <identity>: Registers a new identity at Authorization Authority
- peer_from_sig <authorization_peer_id> <save_file_path>: Adds a new peer from an existing GroupSignature Savefile
- get_opener_keys <authorization_peer_id>: Requests Opener Keys from Authority
```
## Peer Registration
Each peer needs to be registered to work. This can be done with the `register_identity` command. If a member already has registered his identity,
he can also reuse the group signature of this member identity by using the `peer_from_sig` command with the group_key_save file. Before running the command `peer_from_sig`, the opener keys
have to be fetched with `get_opener_keys`

## Public Groups
Peers can subscribe and unsubscribe to public topics with the `sub` and `unsub` commands.
To publish a message to a topic, the `broad` command can be used.

## Private Groups
Private Groups support all MLS functionality. Peers can create groups, add new members, remove members and update their key material.
For all commands that add new peers to a group the keypackage of the other peer is required, and should be requested with `req_kps`.

Since at the time of writing this thesis no decentralized version of MLS was available, we used the work around of P2Panda. Only Group Owners can create commits.
Therefore, the group owners can not be removed from a group.

But it can delete a group for all members with `delete_group_for_all`.

## Update Group Information
Peers can receive all missed group updates from an archive with the `update_groups` command.

## Future Work
Use a decentralized version of MLS. This would remove the group owner work around mentioned in private groups.