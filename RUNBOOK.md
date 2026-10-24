# DACTIE Multi-Server Runbook

Bring-up and test procedure for the 4-server testbed (minerva1–4), for branch `camera-ready`
and later. Every step is copy-paste bash: run each block on the host named above it.

Last full run: 2026-10-13, all checks in Phase 9 passed.

## Topology

| Host | ssh | IP | Profile | Services | Ports |
| --- | --- | --- | --- | --- | --- |
| minerva1 | `ssh minerva1` | 10.244.192.116 | `node116` | `authority_node` | 10001 |
| minerva2 | `ssh minerva2` | 10.244.192.117 | `node117` | `db`, `adminer`, `archive_node1`, `peer_1` | 5432 and 8080 (loopback), 20011, 21011, 9898 |
| minerva3 | `ssh minerva3` | 10.244.192.118 | `node118` | `db2`, `archive_node2`, `peer_2` | 5433 (loopback), 20012, 21012, 9898 |
| minerva4 | `ssh minerva4` | 10.244.192.119 | `node119` | `peer_3`, `peer_4` | 21013, 21014, 9898, 9899 |

The checkout is `/opt/vero/dactie` on all four hosts. You are in the `docker` group there, so
nothing below needs `sudo` (which would ask for a password).

### Key material map

| Directory | Belongs to | Files |
| --- | --- | --- |
| `key_material_0` | `authority_node` | `key_material.json` + `storage.json` |
| `key_material_1` | `archive_node1` | `key_material.json` |
| `key_material_2` | `archive_node2` | `key_material.json` |
| `key_material_3` | `peer_1` | `key_material.json`, `slots.json`, `group_saves/` |
| `key_material_4` | `peer_2` | same |
| `key_material_5` | `peer_3` | same |
| `key_material_6` | `peer_4` | same |

What the options in `docker-compose.yml` mean is in
[docs/PROTOCOL.md, Running the nodes](docs/PROTOCOL.md#running-the-nodes).

---

## Rules that still matter

1. **The authority needs BOTH `key_material.json` and `storage.json` to keep its
   identity.** If only one of them exists, it refuses to start; with neither, it mints a new
   PeerId, invalidating every `boot_id.env`. Always delete the pair together.
2. **`env_file` is read at container CREATE time.** Editing `boot_id.env` under a running
   container changes nothing: use `up -d --force-recreate`.
3. **Never `stop` a container you just started.** `restart: unless-stopped` does not
   resurrect an explicitly stopped container.
4. **Detach from `attach` with Ctrl-P Ctrl-Q.** Ctrl-C kills the process. The `dsend` helper
   below avoids attaching altogether.
5. **Files written by containers belong to root.** Delete key material through a container
   (Phase 2), and replace root-owned files such as `boot_id.env` with `rm -f` + write, which
   only needs write access to the folder (Phase 4).
6. **A damaged state file is fatal and not self-healing.** Nodes replace `storage.json`,
   `key_material.json`, `slots.json` and `group_saves/state` atomically, so a crash leaves
   the old or the new version, and they refuse to start from a file they cannot read instead
   of starting over. Check `storage.json` with
   `python3 -c "import json;json.load(open('key_material/key_material_0/storage.json'))"`.
   Recovery is Phase 2 and a fresh run.
7. **`end_init_archive` is irreversible.** No archive can join afterwards; adding one means a
   full rerun.

## Helpers

Run this in every minerva shell you open. `dsend` types one line into a container's console
(like attach, type, Ctrl-P Ctrl-Q); `logs` prints a container's log without color codes.

```bash
cd /opt/vero/dactie
dsend() { { printf '%s\r\n' "$2"; sleep 10; } | timeout 5 script -qfec "docker attach --sig-proxy=false $1" /dev/null >/dev/null 2>&1; }
logs()  { docker logs "$@" 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; }
[ -f key_material/boot_id.env ] && export AA=$(sed -n 's/^BOOT_PEER_ID=//p' key_material/boot_id.env)
```

The last line sets `$AA`, the authority's PeerId, once Phase 4 has written it.

---

## Phase 0 — Put the code on the hosts

**Laptop.** The minervas cannot fetch `camera-ready` themselves, so push it over ssh. The
branch must not be checked out on a host while you push it, so switch away first if needed.

```bash
cd ~/projects/dactie-camera
for i in 1 2 3 4; do
  git push ssh://minerva$i/opt/vero/dactie camera-ready:refs/heads/camera-ready
done
```

**All four hosts:**

```bash
cd /opt/vero/dactie
git checkout camera-ready
git log -1 --oneline        # the commit you pushed
```

To update a host that is already on `camera-ready`, push from the laptop to another name,
e.g. `camera-ready:refs/heads/camera-ready-next`, then run `git merge --ff-only camera-ready-next`
on the host.

## Phase 1 — Build

Each host builds only the images it runs. A build takes about 5 minutes per image; run all
four hosts in parallel.

**minerva1:**

```bash
cd /opt/vero/dactie
docker build -t dactie-authority -f ./dactie-authority/Dockerfile .
```

**minerva2 and minerva3:**

```bash
cd /opt/vero/dactie
docker build -t dactie-archive -f ./dactie-archive/Dockerfile .
docker build -t dactie-peer -f ./dactie-peer/Dockerfile . --build-arg FEATURES=bench
```

**minerva4:**

```bash
cd /opt/vero/dactie
docker build -t dactie-peer -f ./dactie-peer/Dockerfile . --build-arg FEATURES=bench
```

The peer image needs `FEATURES=bench` for the benchmark publisher of `peer_4`.

## Phase 2 — Tear down and wipe

**All four hosts.** Stops every container and deletes the key material of the last run.
The key files belong to root (rule 5), so a throwaway container deletes them:

```bash
cd /opt/vero/dactie
docker compose --profile node116 --profile node117 --profile node118 --profile node119 down --remove-orphans
mkdir -p key_material/key_material_{0..9}
docker run --rm -v "$PWD/key_material:/km" busybox sh -c \
  'rm -f /km/key_material_[0-9]*/key_material.json /km/key_material_[0-9]*/storage.json /km/key_material_[0-9]*/slots.json; rm -rf /km/key_material_[0-9]*/group_saves'
find key_material/key_material_[0-9]* \( -name key_material.json -o -name storage.json -o -name slots.json -o -name group_saves \)   # expect no output
```

**minerva2** and **minerva3** additionally empty the archive's database, which still holds the
last run's messages and shares:

```bash
docker volume rm dactie_db_data      # minerva2
docker volume rm dactie_db2_data     # minerva3
```

Optional, before wiping: keep a copy of the old state.

```bash
mkdir -p /opt/vero/backups
tar czf /opt/vero/backups/key_material-$(date +%Y%m%d).tgz key_material
docker run --rm -v dactie_db_data:/data:ro -v /opt/vero/backups:/backup busybox \
  tar czf /backup/dactie_db_data-$(date +%Y%m%d).tgz -C /data .     # minerva2; dactie_db2_data on minerva3
```

## Phase 3 — Authority

**minerva1:**

```bash
docker compose --profile node116 up -d authority_node
sleep 6
logs dactie-authority_node-1 | grep -E "Local PeerId|Archives registered"
```

Expect `Archives registered: 0, init phase: open` and a `Local PeerId: 12D3KooW…` line.
`init phase: CLOSED` means the key material was not wiped: go back to Phase 2.

## Phase 4 — Tell every host the authority's PeerId

**Laptop.** Reads the PeerId on minerva1 and writes `boot_id.env` on all four hosts. The old
file may belong to root (rule 5), so it is removed first.

```bash
AA=$(ssh minerva1 'docker logs dactie-authority_node-1 2>&1' | grep -o 'Local PeerId: [A-Za-z0-9]*' | tail -1 | awk '{print $3}')
echo "$AA"
for i in 1 2 3 4; do
  ssh minerva$i "cd /opt/vero/dactie && rm -f key_material/boot_id.env && echo BOOT_PEER_ID=$AA > key_material/boot_id.env && cat key_material/boot_id.env"
done
```

Then rerun the [Helpers](#helpers) block in every minerva shell, so `$AA` is set there.

In each of your four minerva terminals (minerva2–4), not the AA, paste these lines:

```bash
cd /opt/vero/dactie
export AA=$(sed -n 's/^BOOT_PEER_ID=//p' key_material/boot_id.env)
echo $AA
```

Or the helpers block again:

```bash
cd /opt/vero/dactie
dsend() { { printf '%s\r\n' "$2"; sleep 10; } | timeout 5 script -qfec "docker attach --sig-proxy=false $1" /dev/null >/dev/null 2>&1; }
logs()  { docker logs "$@" 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; }
[ -f key_material/boot_id.env ] && export AA=$(sed -n 's/^BOOT_PEER_ID=//p' key_material/boot_id.env)
```

## Phase 5 — Archives

The archives' database password has no default. On minerva2 and minerva3, write it once into
`/opt/vero/dactie/.env`, where compose reads it, and keep it as long as the database volume:
PostgreSQL stores it when the volume is created (Phase 2 deletes the volume).

```bash
[ -f .env ] || { umask 077; echo "ARCHIVE_DB_PASSWORD=$(openssl rand -hex 16)" > .env; }
```

**minerva2:**

```bash
docker compose --profile node117 up -d --force-recreate db
sleep 10
docker compose --profile node117 up -d --force-recreate archive_node1
```

**minerva3:**

```bash
docker compose --profile node118 up -d --force-recreate db2
sleep 10
docker compose --profile node118 up -d --force-recreate archive_node2
```

PostgreSQL first: an archive exits at startup if its database is unreachable.

## Phase 6 — Confirm registration

**minerva1:**

```bash
logs dactie-authority_node-1 | grep "Verified identity archive"
```

Expect two lines, `Verified identity archive1 from 12D3KooW…` and `… archive2 from …`. If one
is missing, check that archive's log for the PeerId it dials
(`logs dactie-archive_node1-1 | grep "Registering identity"`): an old PeerId means its
`boot_id.env` was not replaced (Phase 4) before the container was created.

## Phase 7 — Close the init phase

**minerva1:**

```bash
dsend dactie-authority_node-1 end_init_archive
sleep 3
logs --since 1m dactie-authority_node-1 | grep -E "Enough Archives|Creating common group|Admitted"
```

Expect all three:

```text
INFO  Enough Archives registered, ending init phase (opening threshold 2 of 2)
INFO  Creating common group with archives...
INFO  Admitted 3 PeerIDs to GossipSub
```

If only the first line appears, the authority is wedged: a full rerun from Phase 2 is needed.

## Phase 8 — Peers

`register_identity` joins from a throwaway identity and then registers the peer's own
PeerId as a sharing peer, so wait a few seconds before `sub`.

**minerva2:**

```bash
docker compose --profile node117 up -d --force-recreate adminer peer_1
sleep 5
dsend dactie-peer_1-1 "register_identity $AA peer1"
dsend dactie-peer_1-1 "sub Test"
```

**minerva3:**

```bash
docker compose --profile node118 up -d --force-recreate peer_2
sleep 5
dsend dactie-peer_2-1 "register_identity $AA peer2"
dsend dactie-peer_2-1 "sub Test"
```

**minerva4:**

```bash
docker compose --profile node119 up -d --force-recreate peer_3 peer_4
sleep 5
dsend dactie-peer_3-1 "register_identity $AA peer3"
dsend dactie-peer_4-1 "register_identity $AA peer4"
dsend dactie-peer_3-1 "sub Test"
dsend dactie-peer_4-1 "sub Test"
```

**minerva1** confirms all four:

```bash
logs dactie-authority_node-1 | grep -E "Verified identity peer|Created Key Material|Adding new peer|Admitted"
```

Expect, per peer, `Verified identity peerN from <PeerId>`, `Created Key Material for member
<MemberID>` and `Adding new peer from existing member`, ending with `Admitted 7 PeerIDs to
GossipSub` (authority, 2 archives, 4 peers). `peer_4` then starts its benchmark traffic.

---

## Phase 9 — Checks

### 9.1 The join came from a throwaway identity

Each peer prints its own PeerId and the throwaway one it joined from, e.g. on **minerva2**:

```bash
logs dactie-peer_1-1 | grep -o -E 'Local PeerId: [A-Za-z0-9]*|Joining from throwaway identity [A-Za-z0-9]*|Registering with slot [0-9]+ of epoch [0-9]+'
```

The two PeerIds must differ, and the `Verified identity peer1 from …` line on minerva1 must
name the throwaway one: the authority never links a peer's own PeerId to a name. Expect
`Registering with slot 1 of epoch …`. The same holds for `dactie-peer_2-1` on minerva3 and
`dactie-peer_3-1`, `dactie-peer_4-1` on minerva4.

### 9.2 Anonymous broadcast

**minerva3:**

```bash
dsend dactie-peer_2-1 "broad Test hello-from-peer2"
```

**minerva2:**

```bash
logs --since 1m dactie-peer_1-1 | grep -A5 "hello-from-peer2"
logs --since 1m dactie-peer_1-1 | grep -A5 "hello-from-peer2" | grep -A1 '^Signature:' | tail -1 | tr -d '[:space:]' > /tmp/sig.hex
```

Expect the message with `Sender: anonymous`. The second command keeps its signature for 9.6.

### 9.3 Attributed broadcast

**minerva3:**

```bash
dsend dactie-peer_2-1 "attr_broad Test signed-by-peer2"
```

**minerva2:**

```bash
logs --since 1m dactie-peer_1-1 | grep -A5 "signed-by-peer2"
```

Expect `Attributed to member: <MemberID>`, the MemberID that minerva1 logged as `Created Key
Material for member …` for `peer2` (the second one).

### 9.4 Benchmark traffic

**minerva4:**

```bash
logs dactie-peer_4-1 | grep -c "Publishing gossipsub payload"
logs dactie-peer_3-1 | grep -c BENCH_RX
logs dactie-peer_3-1 | grep BENCH_RX | tail -1
```

Both counts climb by about 400 every 10 s (40 msg/s). A `BENCH_RX` line ends with
`latency_ms=<ms>`; the last run showed 11 ms.

### 9.5 An unregistered node gets nothing

**minerva4:** a peer that never registers subscribes to the benchmark topic:

```bash
docker run -dit --name dactie-observer --network host -e RUST_LOG=info dactie-peer \
  sh -c "/usr/src/dactie/target/release/dactie-peer -K /tmp/obs -b $AA -B /ip4/10.244.192.116/tcp/10001 -p 21099 --advertise-addr /ip4/10.244.192.119/tcp/21099"
sleep 6
dsend dactie-observer "sub Test"
sleep 30
logs dactie-observer | grep -c "Received gossipsub payload"      # expect 0
export OBS=$(logs dactie-observer | grep -o 'Local PeerId: [A-Za-z0-9]*' | head -1 | awk '{print $3}'); echo $OBS
```

`peer_3` receives benchmark messages meanwhile (9.4); the observer must not. After 60 s every
node it connected to closes the connection:

```bash
sleep 40
logs dactie-peer_3-1 | grep -c "Closing connection to $OBS"      # expect 1
docker rm -f dactie-observer
```

**minerva4:** print the observer's PeerId and copy it:

```bash
echo $OBS
```

**minerva1:** paste it and look for the authority's line about it:

```bash
export OBS=<PeerId from minerva4>
logs dactie-authority_node-1 | grep "Closing connection to $OBS"
```

Expect `Closing connection to <OBS>: not admitted within 60s`.

### 9.6 Opening

**minerva2** prints the signature from 9.2; copy it:

```bash
cat /tmp/sig.hex
```

**minerva1:**

```bash
export SIG=<signature from minerva2>
dsend dactie-authority_node-1 "req_open $SIG"
```

**minerva2**, then **minerva3** with `dactie-archive_node2-1`: each archive operator approves.

```bash
ID=$(logs dactie-archive_node1-1 | grep -o 'Opening request [0-9a-f]*' | tail -1 | awk '{print $3}'); echo $ID
dsend dactie-archive_node1-1 "approve $ID"
logs --since 1m dactie-archive_node1-1 | grep "Submitted this archive's partial opening"
```

**minerva1:**

```bash
dsend dactie-authority_node-1 "open $SIG"
sleep 3
logs --since 1m dactie-authority_node-1 | grep "Signature belongs"
```

Expect `Signature belongs to ID:<MemberID>, Name:peer2` and peer_2's own PeerId (9.1), not its
throwaway one.

### 9.7 A second sharing peer of the same member

**minerva2:** a second node of member `peer1` uses peer_1's key file and takes slot 2.

```bash
docker run -dit --name dactie-peer1b --network host -e RUST_LOG=info \
  -v /opt/vero/dactie/key_material/key_material_3:/member dactie-peer \
  sh -c "/usr/src/dactie/target/release/dactie-peer -K /tmp/peer1b -b $AA -B /ip4/10.244.192.116/tcp/10001 -p 21021 --advertise-addr /ip4/10.244.192.117/tcp/21021"
sleep 6
dsend dactie-peer1b "get_opener_keys $AA"
dsend dactie-peer1b "peer_from_sig $AA /member/key_material.json"
logs dactie-peer1b | grep -E "Registering with slot|refused"
cat key_material/key_material_3/slots.json
docker rm -f dactie-peer1b
```

Expect `Registering with slot 2 of epoch …`, `slots.json` with slots `1` and `2` for that
epoch, and a fifth `Adding new peer from existing member` on minerva1.

---

## Variant: expiry with 10-minute epochs

Epochs last 30 days by default. To see expiry, run Phases 2–8 again with one change in
**Phase 3** on **minerva1**: start the authority with short epochs.

```bash
cat > /tmp/epoch-test.yml <<'EOF'
services:
  authority_node:
    command: >-
      sh -c '/usr/src/dactie/target/release/dactie-authority
      -K /usr/src/dactie/key_material/
      -p 10001
      --advertise-addr /ip4/10.244.192.116/tcp/10001
      --epoch-secs 600
      --peers-per-member 3'
EOF
docker compose -f docker-compose.yml -f /tmp/epoch-test.yml --profile node116 up -d authority_node
```

Epochs start at multiples of 600 s of Unix time (`echo $(( 600 - $(date +%s) % 600 ))` prints
the seconds left). Before the epoch ends, renew peer_1 and peer_2 but not peer_3 and peer_4:

```bash
dsend dactie-peer_1-1 "renew $AA /usr/src/dactie/key_material/key_material.json"     # minerva2
dsend dactie-peer_2-1 "renew $AA /usr/src/dactie/key_material/key_material.json"     # minerva3
```

**minerva1:**

```bash
logs dactie-authority_node-1 | grep -E "Renewed sharing peer|Expired"
```

Expect two `Renewed sharing peer … until epoch <E+1>` lines, and after the boundary
`Expired 2 sharing peers: [...]` with the PeerIds of peer_3 and peer_4. From then on they are
out of every mesh: on **minerva4**, `logs dactie-peer_3-1 | grep -c BENCH_RX` stops climbing.

## Tear down

**All four hosts:**

```bash
cd /opt/vero/dactie
docker compose --profile node116 --profile node117 --profile node118 --profile node119 down --remove-orphans
docker rm -f dactie-observer dactie-peer1b 2>/dev/null
```

---

## Expected noise

- `HEARTBEAT: Mesh low. Topic contains: 1 needs: 5` / `RANDOM PEERS: Got 0 peers` at DEBUG:
  an empty GossipSub mesh, normal before peers subscribe.
- `Dial error: Unexpected peer ID`: a stale Kademlia entry for an identity wiped after the
  authority had met it. Phase 2 on all hosts prevents it.
- `Error: BuildError(... AddrInUse ...)` in a burst right after `up -d --force-recreate`: the
  new process binds before the old one released the port (`network_mode: host`).
  `restart: unless-stopped` retries; a restart count in the single digits is expected. A
  climbing count means something outside compose holds the port, or a corrupt `storage.json`
  (rule 6):

  ```bash
  docker inspect -f '{{.RestartCount}} {{.State.Status}}' dactie-authority_node-1
  ```

- `Closing connection to <PeerId>: not admitted within 60s` and `Refusing connection from
  <PeerId>: not admitted`: a node that is not in the common group, e.g. a peer during its join
  or a stale one. Expected and harmless.
- Log lines contain color codes; `grep` still works, and the `logs` helper strips them.

## Older builds

On `main` before `camera-ready`, peers registered with their own PeerId (no throwaway
identity), and `register_identity` added that PeerId to the common group directly; there are no
slot tags, no admission checks and no attribution, so Phase 9 does not apply. The archive
self-healing and `build.sh` fixes are present since `hardening-docker-runs`.
