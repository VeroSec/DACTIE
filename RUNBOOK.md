# DACTIE Multi-Server Runbook

Bring-up procedure for the 4-server testbed.

Applies to branch `hardening-docker-runs` and later. On `main`/`merge-test` the archives
do **not** self-heal — see [Older builds](#older-builds) at the end.

## Topology

| Server | IP | Profile | Services | Ports |
|---|---|---|---|---|
| Server1 | 10.244.192.116 | `node116` | `authority_node` | 10001 |
| Server2 | 10.244.192.117 | `node117` | `db`, `adminer`, `archive_node1`, `peer_1` | 5432, 8080, 20011, 21011, 9898 |
| Server3 | 10.244.192.118 | `node118` | `db2`, `archive_node2`, `peer_2` | 5433, 20012, 21012, 9898 |
| Server4 | 10.244.192.119 | `node119` | `peer_3`, `peer_4` | 21013, 21014, 9898, 9899 |

### Key material map

| Directory | Belongs to | Files |
|---|---|---|
| `key_material_0` | `authority_node` | `key_material.json` + `storage.json` |
| `key_material_1` | `archive_node1` | `key_material.json` |
| `key_material_2` | `archive_node2` | `key_material.json` |
| `key_material_3` | `peer_1` | `key_material.json` |
| `key_material_4` | `peer_2` | `key_material.json` |
| `key_material_5` | `peer_3` | `key_material.json` |
| `key_material_6` | `peer_4` | `key_material.json` |

---

## Rules that still matter

1. **The authority needs BOTH `key_material.json` and `storage.json` to keep its
   identity.** `dactie-authority/src/main.rs` reuses the keypair only when both exist;
   delete one and it mints a new PeerId, invalidating every `boot_id.env`. Always delete
   the pair together.
2. **`env_file` is read at container CREATE time.** Editing `boot_id.env` under a running
   container changes nothing — use `up -d --force-recreate`.
3. **Never `stop` a container you just started.** `restart: unless-stopped` deliberately
   does not resurrect an explicitly stopped container.
4. **Detach from `attach` with Ctrl-P Ctrl-Q.** Ctrl-C sends SIGINT; the binaries handle
   only SIGTERM, so Ctrl-C kills the process.
5. **A truncated `storage.json` is fatal and not self-healing.** The authority
   fails to parse it at startup and, under `restart: unless-stopped`, crash-loops
   silently. Check with
   `python3 -c "import json;json.load(open('key_material/key_material_0/storage.json'))"`.
   Recovery is Phase 2 plus a fresh Phase 3/4, since the authority identity is lost.
6. **`end_init_archive` is irreversible.** It sets `archive_init_mode = false` and
   persists it. No archive can join afterwards; adding one means a full rerun.

Two former traps are now fixed in code and need no procedure:

- `build.sh` no longer overwrites an existing `boot_id.env`, and forwards its arguments,
  so `./build.sh --no-cache --pull` works.
- Archives re-register automatically whenever they hold no group key, so a failed
  registration is no longer permanent.

---

## Phase 0 — Tear down

**All four servers**, in parallel.

```bash
cd /path/to/dactie
sudo docker compose --profile node116 --profile node117 --profile node118 --profile node119 down --remove-orphans
docker ps -a | grep dactie      # expect no output
```

## Phase 1 — Build

**All four servers**, in parallel.

```bash
cd /path/to/dactie
git checkout hardening-docker-runs
sudo ./build.sh --no-cache --pull
```

## Phase 2 — Wipe key material

**All four servers**, in parallel. Only needed for a clean slate (new authority identity);
skip it to keep existing identities.

```bash
cd /path/to/dactie
sudo rm -f  ./key_material/key_material_[0-9]*/key_material.json \
            ./key_material/key_material_[0-9]*/storage.json
sudo rm -rf ./key_material/key_material_[0-9]*/group_saves
```

Both `.json` files go together (rule 1). Wiping every node in one pass also keeps stale
identities out of the DHT.

## Phase 3 — Authority

**Server1 (.116) alone.**

```bash
cd /path/to/dactie
sudo docker compose --profile node116 up -d authority_node
sleep 5
sudo docker compose --profile node116 logs authority_node \
  | grep -E "Local PeerId|Listening on|Archives registered"
ss -tlnp | grep 10001
```

You now get an explicit state line:

```
INFO  Archives registered: 0, init phase: open
```

`init phase: CLOSED` means this key material has already completed a run — go back to
Phase 2 or you will never register an archive.

Copy the `Local PeerId` value.

> **BARRIER.** Do not touch Servers 2-4 until the PeerId is known and 10001 is listening.

## Phase 4 — Propagate the authority ID

**All four servers**, after Phase 1.

```bash
cd /path/to/dactie
export AA=<paste_Local_PeerId_from_Phase_3>
echo "BOOT_PEER_ID=$AA" | sudo tee ./key_material/boot_id.env
```

Keep `$AA` in the shell — Phase 8 needs it.

## Phase 5 — Archives

**Server2 (.117):**
```bash
sudo docker compose --profile node117 up -d --force-recreate db
sleep 10
sudo docker compose --profile node117 up -d --force-recreate archive_node1
sudo docker compose --profile node117 logs archive_node1 | grep -E "Registering identity|Group key|Postgres"
```

**Server3 (.118):**
```bash
sudo docker compose --profile node118 up -d --force-recreate db2
sleep 10
sudo docker compose --profile node118 up -d --force-recreate archive_node2
sudo docker compose --profile node118 logs archive_node2 | grep -E "Registering identity|Group key|Postgres"
```

Postgres first — an archive exits at startup if its DSN is unreachable.

Each archive logs its decision explicitly:

| Log line | Meaning |
|---|---|
| `Registering identity 'archiveN' with authority ...` | Registration sent |
| `Key material has no group key, re-registering` | Previous attempt failed, retrying |
| `Group key present, skipping registration` | Already registered, nothing to do |

If an archive reports the third line but the authority shows it as unregistered, its key
material belongs to a different authority — wipe that node's `key_material.json` and
recreate it.

## Phase 6 — Confirm registration

**Server1 (.116).** The authority now announces every registration:

```bash
sudo docker compose --profile node116 logs authority_node | grep "registered"
```

Wait for **two** distinct lines:

```
INFO  Archive '12D3KooW...' registered (1 total)
INFO  Archive '12D3KooW...' registered (2 total)
```

Do not continue on one. If an archive is missing, fix it in Phase 5 — the authority side
cannot be forced.

## Phase 7 — Close the init phase

**Server1 (.116):**

```bash
sudo docker attach dactie-authority_node-1
```
```
end_init_archive
```

Success requires **both** lines:
```
INFO  Enough Archives registered, ending init phase
INFO  Creating common group with archives...
```

> **STOP if only the first line appears.** The authority is wedged: it stops
> accepting connections, so every later phase fails with misleading peer-side
> errors (`Timeout has been reached`, then `MLS Error: CommonGroup Not Set`).
> Confirm with `sudo ss -tlnp | grep 10001` — a non-zero `Recv-Q` means
> connections are arriving and never being accepted. Recovery is a full rerun
> from Phase 2, because `storage.json` is left truncated. This was a reentrant
> lock in `Storage::return_memkey` and is fixed on this branch; seeing it means
> a node is running an older image.

Failure now reports the count, so you know which problem you have:
```
ERROR Not enough archives registered (1 of 2 required), ...
```

Detach with **Ctrl-P Ctrl-Q**.

Scripting this needs a real terminal — `docker attach` refuses a pipe against a
TTY-enabled container (`cannot attach stdin to a TTY-enabled container because
stdin is not a terminal`). Allocate a pty and hold stdin open past the command:

```bash
sudo sh -c '{ printf "end_init_archive\r\n"; sleep 20; } | \
  timeout 12 script -qfec "docker attach --sig-proxy=false dactie-authority_node-1" /dev/null'
```

`--sig-proxy=false` keeps the timeout from signalling the container, and the
`sleep` prevents EOF reaching its stdin.

## Phase 8 — Peers and smoke test

**Server2 (.117):**
```bash
sudo docker compose --profile node117 up -d --force-recreate adminer peer_1
sudo docker attach dactie-peer_1-1
```
```
register_identity <AA> peer1
sub Test
```

**Server3 (.118):**
```bash
sudo docker compose --profile node118 up -d --force-recreate peer_2
sudo docker attach dactie-peer_2-1
```
```
register_identity <AA> peer2
sub Test
broad Test hello-from-peer2
```

**Server4 (.119):**
```bash
sudo docker compose --profile node119 up -d --force-recreate peer_3 peer_4
sudo docker attach dactie-peer_3-1
```
```
register_identity <AA> peer3
sub Test
```
```bash
sudo docker attach dactie-peer_4-1
```
```
register_identity <AA> peer4
sub Test
```

**peer_3 and peer_4 must be registered too.** They are not optional extras: a peer
that has not registered has no common group, so `peer_4` starts its BENCH publisher
immediately and every publish fails with `MLS Error: CommonGroup Not Set`. The
publisher does not wait for registration, so those errors appear before you have a
chance to register — they stop once registration completes, and the run is only
valid from that point on.

`peer1` should print the message.

Once all four peers are registered, confirm the benchmark is actually moving
traffic rather than erroring:

```bash
sudo docker compose --profile node119 logs peer_4 | grep -c "Publishing gossipsub payload"
sudo docker compose --profile node119 logs peer_3 | grep -c BENCH_RX
```

Both counts should climb. A `BENCH_RX` line reads
`topic=Test via=<relay> seq=<n> latency_ms=<ms>` — `via` is the relaying
neighbour, not the originator, which is deliberate: the probe payload carries no
sender id.

Confirm the BENCH publisher is actually compiled in — it lives behind the
`dactie-peer/bench` feature, which `build.sh` passes as a build arg. If this line
is missing from `peer_4`, the image predates that change and no benchmark traffic
will be produced at all:

```bash
sudo docker compose --profile node119 logs peer_4 | grep BENCH_PUBLISH
# INFO  BENCH_PUBLISH enabled: rate=40 msg/s topic='Test' extra_bytes=0
```
 Substitute the literal ID for `<AA>` — container stdin
does not expand shell variables. `peer_4` runs with `BENCH_PUBLISH=true`, `BENCH_RATE=40`,
`BENCH_TOPIC=Test`, so it self-publishes without input.

Adminer: `http://10.244.192.117:8080` — user `archive`, password `archivepw`, database
`archivedb`.

---

## Expected noise

- `HEARTBEAT: Mesh low. Topic contains: 1 needs: 5` / `RANDOM PEERS: Got 0 peers` at DEBUG
  — an empty gossipsub mesh. Normal before peers subscribe; if it persists after Phase 8,
  that node is genuinely isolated.
- `Dial error: Unexpected peer ID` — a stale Kademlia entry for an identity wiped after
  the authority had already met it. Following Phase 2 prevents these; existing ones decay
  as Kademlia evicts them.

`Connection refused` against `172.17.0.1` / `172.18.0.1` should no longer appear —
`dactie_utils::net::is_unroutable_addr` now keeps Docker bridge addresses out of the DHT.
If you still see them, one node is running a pre-hardening image.

## Older builds

On `main`/`merge-test`, three extra steps are required:

1. `build.sh` resets `boot_id.env` to `<insert_here>` on every run, so Phase 4 must always
   follow Phase 1 — never the reverse.
2. `build.sh` ignores its arguments; use direct `docker build --no-cache --pull -t ...`
   calls for a clean rebuild.
3. An archive registers **only** when `key_material.json` is absent, and writes that file
   before the authority answers. A registration that fails is permanent and silent. Before
   every archive start, delete `key_material.json` from `key_material_1` / `key_material_2`
   by hand, and make sure the authority is already listening.
