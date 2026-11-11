# dactie-utils

Code shared by the authority, the archives and the sharing peers.

| Module | Contents |
| --- | --- |
| `admission` | Restricts GossipSub to admitted nodes, i.e. the PeerIDs in the common MLS group |
| `conn_cap` | Connection limits that refuse a connection before the handshake |
| `dedup` | Gossip messages already handled, kept far longer than GossipSub's own duplicate cache |
| `key_store` | A node's key material, saved so that it can restart |
| `mls_wrapper` | MLS groups (common and private), encryption and channels |
| `net` | GossipSub mesh parameters and address helpers |
| `persist` | Crash-safe writes of node state |
| `shared_structs` | The messages the node types exchange |
| `slot_store` | The slots a member has used per epoch (`slots.json`) |
| `stem` | The stem phase (Dandelion++) for sharing peers |

How these pieces fit together is described in [docs/PROTOCOL.md](../../docs/PROTOCOL.md).
