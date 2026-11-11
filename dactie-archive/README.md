# dactie-archive

The archive (ARC): it subscribes to all channels and stores every message in its own
PostgreSQL database, holds one share of every member key, and serves missed group updates to
peers that were offline. Its operator approves or denies every request to open a signature.

- **Run it:** the [walkthrough](../README.md#walkthrough-run-a-network-by-hand) in the main
  README. It needs a PostgreSQL database with the tables of [init.sql](../init.sql); pass the
  password in `PGPASSWORD`, not in the URL.
- **Options:** [docs/PROTOCOL.md, "dactie-archive"](../docs/PROTOCOL.md#dactie-archive).
- **Console:** `pending`, `approve <id>`, `deny <id>`
  ([README, "Console commands"](../README.md#console-commands)).
- **What it stores and when it opens:** [docs/PROTOCOL.md](../docs/PROTOCOL.md), sections 1, 6
  and 7.

## Code

| File | Contents |
| --- | --- |
| `src/main.rs` | Command-line options, start-up, database connection |
| `src/myclient.rs` | Operator console |
| `src/mybehaviour.rs` | Network behaviour: registration, storage, key shares, partial openings |
