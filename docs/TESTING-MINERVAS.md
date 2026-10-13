# Testing `camera-ready` on the minervas

Step-by-step test of the camera-ready changes on the 4-server testbed, with copy-paste bash
for every step. The phases follow [RUNBOOK.md](../RUNBOOK.md), whose rules (both authority
files together, `end_init_archive` is irreversible, detach with Ctrl-P Ctrl-Q, …) still apply.

What this checks, beyond the old smoke test:

| Change | Check |
| --- | --- |
| Join from a throwaway identity (#3) | The AA links the name to a PeerID the peer never uses again; the peer then registers its own PeerID as a sharing peer |
| Slot tags | Each join uses slot 1 of its member; a second sharing peer of the same member uses slot 2 |
| Admission to GossipSub | An unregistered node receives no gossip, and the AA closes its connection after 60 s |
| Attribution (#5) | `attr_broad` shows the sender's MemberID, `broad` shows `Sender: anonymous` |
| Opening | Names the member and lists all its sharing PeerIDs |
| Expiry (optional, separate run) | A sharing peer that does not renew is removed when its epoch ends |

## Hosts

| Host | ssh | IP | Profile | Services |
| --- | --- | --- | --- | --- |
| minerva1 | `ssh minerva1` | 10.244.192.116 | `node116` | `authority_node` |
| minerva2 | `ssh minerva2` | 10.244.192.117 | `node117` | `db`, `adminer`, `archive_node1`, `peer_1` |
| minerva3 | `ssh minerva3` | 10.244.192.118 | `node118` | `db2`, `archive_node2`, `peer_2` |
| minerva4 | `ssh minerva4` | 10.244.192.119 | `node119` | `peer_3`, `peer_4` |

The checkout is `/opt/vero/dactie` on all four. You are in the `docker` group there, so
no step needs `sudo`.

## Helper: send a console command without attaching

Run this once in every minerva shell you use. It types one line into a container's console,
like `docker attach` + typing + Ctrl-P Ctrl-Q:

```bash
cd /opt/vero/dactie
dsend() { { printf '%s\r\n' "$2"; sleep 10; } | timeout 5 script -qfec "docker attach --sig-proxy=false $1" /dev/null >/dev/null; }
```

Read the answer from the logs, e.g. `docker logs --since 1m dactie-peer_1-1 2>&1 | tail`.
If `dsend` does nothing, attach by hand: `docker attach <container>`, type the line,
detach with **Ctrl-P Ctrl-Q** (never Ctrl-C).

---

## Phase 1 — Put `camera-ready` on the minervas

**Laptop.** The minervas have no access to the branch, so push it to each of them over ssh.
The branch is not checked out there, so the push is accepted:

```bash
cd ~/projects/dactie-camera
for i in 1 2 3 4; do
  git push ssh://minerva$i/opt/vero/dactie camera-ready:refs/heads/camera-ready
done
```

**All four minervas**, in parallel: stop the old containers and switch the checkout.
`key_material/` is not tracked, so the switch leaves it alone.

```bash
cd /opt/vero/dactie
docker compose --profile node116 --profile node117 --profile node118 --profile node119 down --remove-orphans
git checkout camera-ready
git log -1 --oneline        # expect the commit you pushed, e.g. 5546ea4
```

## Phase 2 — Build

Each host builds only the images it runs, with the flags `build.sh` uses. Allow about
half an hour per host on the 4-core VMs; run all four in parallel.

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

`./build.sh` builds all three images on a host instead, if you prefer.

## Phase 3 — Wipe key material

**All four minervas.** The old key material belongs to the pre-camera-ready protocol, so
start clean. The files belong to your user; if `rm` reports `Permission denied`, prefix it
with `sudo`.

```bash
cd /opt/vero/dactie
mkdir -p key_material/key_material_{0..9}
rm -f  key_material/key_material_[0-9]*/key_material.json \
       key_material/key_material_[0-9]*/storage.json \
       key_material/key_material_[0-9]*/slots.json
rm -rf key_material/key_material_[0-9]*/group_saves
```

The archives' PostgreSQL databases still hold the old run's messages and shares. For a
clean slate there too, remove their volumes on **minerva2** and **minerva3**:

```bash
cd /opt/vero/dactie
docker volume ls | grep -E 'db_data|db2_data'
docker volume rm dactie_db_data 2>/dev/null; docker volume rm dactie_db2_data 2>/dev/null
```

## Phase 4 — Authority

**minerva1:**

```bash
cd /opt/vero/dactie
docker compose --profile node116 up -d authority_node
sleep 5
docker logs dactie-authority_node-1 2>&1 | grep -E "Local PeerId|Archives registered"
```

Expect `Archives registered: 0, init phase: open`. Copy the `Local PeerId`:

```bash
export AA=$(docker logs dactie-authority_node-1 2>&1 | grep -o 'Local PeerId: [A-Za-z0-9]*' | tail -1 | awk '{print $3}')
echo $AA
```

## Phase 5 — Tell every host the authority's PeerId

**All four minervas.** Paste the value from Phase 4:

```bash
cd /opt/vero/dactie
export AA=<PeerId from Phase 4>
echo "BOOT_PEER_ID=$AA" > key_material/boot_id.env
```

Keep `$AA` set in these shells; the later phases use it.

## Phase 6 — Archives

**minerva2:**
```bash
cd /opt/vero/dactie
docker compose --profile node117 up -d --force-recreate db
sleep 10
docker compose --profile node117 up -d --force-recreate archive_node1
```

**minerva3:**
```bash
cd /opt/vero/dactie
docker compose --profile node118 up -d --force-recreate db2
sleep 10
docker compose --profile node118 up -d --force-recreate archive_node2
```

**minerva1:** wait for both archives:

```bash
docker logs dactie-authority_node-1 2>&1 | grep -E "Verified identity archive"
```

Expect two lines, `Verified identity archive1 from …` and `… archive2 from …`.

## Phase 7 — Close the init phase

**minerva1:**

```bash
dsend dactie-authority_node-1 end_init_archive
sleep 3
docker logs --since 1m dactie-authority_node-1 2>&1 | grep -E "Enough Archives registered|Creating common group|Admitted"
```

Expect `Enough Archives registered, ending init phase (opening threshold 2 of 2)` and
`Creating common group with archives...`. If only the first line appears, see Phase 7 of
RUNBOOK.md.

## Phase 8 — Peers join

Start each peer and register it. `register_identity` now joins from a throwaway identity
and then registers the peer's own PeerID as a sharing peer, so it takes a few seconds longer.

**minerva2:**
```bash
docker compose --profile node117 up -d --force-recreate adminer peer_1
sleep 5
dsend dactie-peer_1-1 "register_identity $AA peer1"
sleep 8
dsend dactie-peer_1-1 "sub Test"
docker logs dactie-peer_1-1 2>&1 | grep -E "Local PeerId|Joining from throwaway|Joined as a member|Registering with slot"
```

**minerva3:**
```bash
docker compose --profile node118 up -d --force-recreate peer_2
sleep 5
dsend dactie-peer_2-1 "register_identity $AA peer2"
sleep 8
dsend dactie-peer_2-1 "sub Test"
```

**minerva4:**
```bash
docker compose --profile node119 up -d --force-recreate peer_3 peer_4
sleep 5
dsend dactie-peer_3-1 "register_identity $AA peer3"
dsend dactie-peer_4-1 "register_identity $AA peer4"
sleep 8
dsend dactie-peer_3-1 "sub Test"
dsend dactie-peer_4-1 "sub Test"
```

`peer_4` starts publishing benchmark traffic on `Test` once it is registered.

### Check: the join used a throwaway identity

**minerva2** prints peer_1's own PeerID and the throwaway one it joined from:

```bash
docker logs dactie-peer_1-1 2>&1 | grep -E "Local PeerId|Joining from throwaway identity|Registering with slot"
```

Expect `Registering with slot 1 of epoch …`. On **minerva1**, the name `peer1` must be linked
to the throwaway PeerID, never to peer_1's own:

```bash
docker logs dactie-authority_node-1 2>&1 | grep -E "Verified identity peer|Created Key Material for member|Adding new peer from existing member"
```

Expect one `Verified identity peerN from <throwaway>` and one `Adding new peer from existing
member` per peer: four of each.

## Phase 9 — Tests

### 9.1 Anonymous broadcast

**minerva3:**
```bash
dsend dactie-peer_2-1 "broad Test hello-from-peer2"
```

**minerva2:**
```bash
docker logs --since 1m dactie-peer_1-1 2>&1 | grep -A5 "hello-from-peer2"
```

Expect the message with `Sender: anonymous` and a `Signature:` block. Copy the signature
hex for 9.5.

### 9.2 Attributed broadcast

**minerva3:**
```bash
dsend dactie-peer_2-1 "attr_broad Test signed-by-peer2"
```

**minerva2:**
```bash
docker logs --since 1m dactie-peer_1-1 2>&1 | grep -A5 "signed-by-peer2"
```

Expect `Attributed to member: <MemberID>`. **minerva1** shows which member that is:

```bash
docker logs dactie-authority_node-1 2>&1 | grep "Created Key Material for member"
```

The MemberID of peer2 is the second line (members join in the order of Phase 8).

### 9.3 Benchmark traffic

**minerva4:**
```bash
docker logs dactie-peer_4-1 2>&1 | grep BENCH_PUBLISH | head -2
docker logs dactie-peer_4-1 2>&1 | grep -c "Publishing gossipsub payload"
docker logs dactie-peer_3-1 2>&1 | grep -c BENCH_RX
```

Both counts should climb when you repeat the last two commands.

### 9.4 An unregistered node gets nothing

**minerva4:** start a peer that never registers, on its own port, and subscribe it to the
benchmark topic:

```bash
docker run -dit --name dactie-observer --network host dactie-peer \
  sh -c "/usr/src/dactie/target/release/dactie-peer -K /tmp/obs -b $AA -B /ip4/10.244.192.116/tcp/10001 -p 21099 --advertise-addr /ip4/10.244.192.119/tcp/21099"
sleep 5
dsend dactie-observer "sub Test"
sleep 30
docker logs dactie-observer 2>&1 | grep -c "Received gossipsub payload"     # expect 0
OBS=$(docker logs dactie-observer 2>&1 | grep -o 'Local PeerId: [A-Za-z0-9]*' | head -1 | awk '{print $3}'); echo $OBS
```

`peer_3` receives benchmark messages at the same time (9.3); the observer must not.
After 60 s, **minerva1** closes its connection:

```bash
docker logs dactie-authority_node-1 2>&1 | grep "Closing connection to .*: not admitted"
```

Expect a line with the observer's PeerId (`$OBS` on minerva4). Remove it on **minerva4**:

```bash
docker rm -f dactie-observer
```

### 9.5 Opening

**minerva1:** use the signature hex from 9.1:

```bash
export SIG=<signature hex from 9.1>
dsend dactie-authority_node-1 "req_open $SIG"
```

**minerva2** and **minerva3**: each archive operator approves.

```bash
ID=$(docker logs dactie-archive_node1-1 2>&1 | grep -o 'Opening request [0-9a-f]*' | tail -1 | awk '{print $3}'); echo $ID
dsend dactie-archive_node1-1 "approve $ID"
docker logs --since 1m dactie-archive_node1-1 2>&1 | grep "Submitted this archive's partial opening"
```

(on minerva3 the same with `dactie-archive_node2-1`)

**minerva1:**
```bash
dsend dactie-authority_node-1 "open $SIG"
sleep 5
docker logs --since 1m dactie-authority_node-1 2>&1 | grep "Signature belongs"
```

Expect `Signature belongs to ID:<MemberID>, Name:peer2` and peer_2's own PeerID
(`docker logs dactie-peer_2-1 2>&1 | grep 'Local PeerId'` on minerva3). The throwaway PeerID
from the join must not appear.

### 9.6 Optional: a second sharing peer of the same member

**minerva2:** a second node for member `peer1`, using peer_1's key file, takes slot 2:

```bash
docker run -dit --name dactie-peer1b --network host \
  -v /opt/vero/dactie/key_material/key_material_3:/member \
  dactie-peer sh -c "/usr/src/dactie/target/release/dactie-peer -K /tmp/peer1b -b $AA -B /ip4/10.244.192.116/tcp/10001 -p 21021 --advertise-addr /ip4/10.244.192.117/tcp/21021"
sleep 5
dsend dactie-peer1b "get_opener_keys $AA"
sleep 3
dsend dactie-peer1b "peer_from_sig $AA /member/key_material.json"
sleep 5
docker logs dactie-peer1b 2>&1 | grep -E "Registering with slot|refused"
cat key_material/key_material_3/slots.json
```

Expect `Registering with slot 2 of epoch …`, `slots.json` listing slots `1` and `2`, and on
**minerva1** a fifth `Adding new peer from existing member`. Remove it afterwards:
`docker rm -f dactie-peer1b`.

---

## Optional: expiry, as a separate run with 10-minute epochs

Epochs are 30 days by default, so expiry needs its own run. Repeat Phases 3–8 with one
change in **Phase 4** on **minerva1**: start the authority with short epochs.

```bash
cd /opt/vero/dactie
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

Epochs start at multiples of 600 s of Unix time. After Phase 8, **before the epoch ends**,
renew peer_1 and peer_2 but not peer_3. On **minerva2** and **minerva3** respectively:

```bash
dsend dactie-peer_1-1 "renew $AA /usr/src/dactie/key_material/key_material.json"     # minerva2
dsend dactie-peer_2-1 "renew $AA /usr/src/dactie/key_material/key_material.json"     # minerva3
```

**minerva1** confirms, and after the next epoch boundary (`date +%s` divisible by 600)
removes the peers that did not renew:

```bash
docker logs dactie-authority_node-1 2>&1 | grep -E "Renewed sharing peer|Expired"
```

Expect two `Renewed sharing peer … until epoch <E+1>` lines, then `Expired 2 sharing peers:
[...]` with the PeerIDs of peer_3 and peer_4, which did not renew. From then on they are out
of every mesh: on **minerva4**, `docker logs dactie-peer_3-1 2>&1 | grep -c BENCH_RX` stops
climbing. Renew peer_3 and peer_4 as well if you want the benchmark to continue.

---

## Tear down

**All four minervas:**

```bash
cd /opt/vero/dactie
docker compose --profile node116 --profile node117 --profile node118 --profile node119 down --remove-orphans
docker rm -f dactie-observer dactie-peer1b 2>/dev/null
```

To return a host to its previous branch: `git checkout hardening-docker-runs` (minerva2–4)
or `git checkout eclipse-run` (minerva1).

## Troubleshooting

- **`No address for <AA>; start the peer with -B …`** in a peer log: the throwaway join
  cannot dial the authority. Check `-B` in `docker-compose.yml` and that port 10001 listens
  on minerva1 (`ss -tln | grep 10001`).
- **`Authority refused this peer: slot 1 of epoch … was already used by this member`**:
  the member's `slots.json` was lost or the same key material registered twice. Wipe that
  node's key material (Phase 3) and register again.
- **`Epoch length and slots are unknown; run get_opener_keys first`**: run
  `get_opener_keys <AA>` on that peer before `peer_from_sig` or `renew`.
- **A peer gets no messages after registering**: it is admitted only once it is in the
  common group. Check on minerva1 that `Adding new peer from existing member` appeared for
  it, and in the peer log for `Added Group`.
- Everything in RUNBOOK.md's [Expected noise](../RUNBOOK.md#expected-noise) still applies.
