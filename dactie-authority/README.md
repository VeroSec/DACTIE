# dactie-authority

The Authorization Authority (AA): it admits archives and members, fixes the opening threshold
`t` when the archives' init phase ends, owns the common MLS group, registers sharing peers by
their slot tags, and combines the partial openings of `t` archives to open a signature. It
cannot open a signature on its own. Start it first: archives and peers bootstrap from it.

- **Run it:** the [walkthrough](../README.md#walkthrough-run-a-network-by-hand) in the main README.
- **Options:** [docs/PROTOCOL.md, "dactie-authority"](../docs/PROTOCOL.md#dactie-authority).
- **Console:** `end_init_archive`, `req_open <signature hex>`, `open <signature hex>`
  ([README, "Console commands"](../README.md#console-commands)).
- **What it checks and stores:** [docs/PROTOCOL.md](../docs/PROTOCOL.md), sections 1 to 3 and 7.

In the prototype, the authority accepts any unused name at registration; a deployment vets
each organization out of band.

## Code

| File | Contents |
| --- | --- |
| `src/main.rs` | Command-line options, start-up |
| `src/myclient.rs` | Console commands |
| `src/mybehaviour.rs` | Network behaviour: archive init, member and sharing-peer registration, openings |
| `src/storage.rs` | Persistent state (`storage.json`): members, archives, threshold, slot tags |
