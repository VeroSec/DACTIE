# dactie-peer

The sharing peer: it publishes and receives CTI, relays other peers' messages on the stem, and
manages private MLS groups. A member runs up to `k` sharing peers per epoch, none of which the
authority can link to the member.

- **Run it:** the [walkthrough](../README.md#walkthrough-run-a-network-by-hand) in the main README.
- **Options:** [docs/PROTOCOL.md, "dactie-peer"](../docs/PROTOCOL.md#dactie-peer).
- **Console commands:** [README, "Console commands"](../README.md#console-commands). The peer
  prints the list on any unknown command.
- **What it sends and checks:** [docs/PROTOCOL.md](../docs/PROTOCOL.md), sections 2 to 6.

## Code

| File | Contents |
| --- | --- |
| `src/main.rs` | Command-line options, start-up |
| `src/myclient.rs` | Console commands and the printed output of received messages |
| `src/mybehaviour.rs` | Network behaviour: GossipSub, requests, registration, groups |
| `src/join_relay.rs` | The join from a throwaway identity |
| `src/stem_relay.rs` | The stem phase (Dandelion++) as seen by a relay |
| `src/outstanding.rs` | Requests waiting for an answer |

## Private groups

Private groups are nested inside the common group: only admitted peers can be added, and their
messages are encrypted with the private group's key and then with the common group's key. Add
members after fetching their key packages with `req_kps`.

No decentralized version of MLS was available, so DACTIE uses P2Panda's workaround: only the
group owner creates commits. The owner therefore cannot be removed from its group, but can
delete it for all members with `delete_group_for_all`. It also removes the peers the authority
removed from the common group.
