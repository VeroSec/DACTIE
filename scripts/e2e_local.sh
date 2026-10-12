#!/bin/bash
# Local end-to-end run: join, an additional sharing peer, a broadcast, and an opening.
# AA, two archives (PostgreSQL in Docker), member peer1, and peer2 as a second sharing peer
# of the same member (peer_from_sig). peer2 publishes, both archives approve the opening,
# and the AA must name peer1's member and list peer2's PeerID.
# Needs Docker, tmux and `cargo build --release`. Logs go to target/e2e/.
# Leaves tmux session "camera" and containers camera-pg1/2 running for inspection;
# clean up with: tmux kill-session -t camera; docker rm -f camera-pg1 camera-pg2
set -uo pipefail
REPO=${REPO:-$(cd "$(dirname "$0")/.." && pwd)}
BIN=$REPO/target/release
W=${W:-$REPO/target/e2e}
S=camera
AA_PORT=10001

rm -rf "$W"; mkdir -p "$W"/{aa,arc1,arc2,p1,p2}
tmux kill-session -t $S 2>/dev/null
docker rm -f camera-pg1 camera-pg2 >/dev/null 2>&1

fail() { echo "FAIL: $*"; exit 1; }
wait_count() { # file pattern count seconds
    for _ in $(seq 1 "$4"); do
        n=$(grep -c -- "$2" "$1" 2>/dev/null)
        [ "${n:-0}" -ge "$3" ] && return 0
        sleep 1
    done
    fail "waited $4 s for $3x '$2' in $(basename "$1")"
}
run() { tmux new-window -d -t $S -n "$1" "cd $W && RUST_LOG=info $2 2>&1 | tee $W/$1.log"; }
send() { tmux send-keys -t "$S:$1" "$2" Enter; }

for i in 1 2; do
    docker run -d --rm --name camera-pg$i -e POSTGRES_USER=archive -e POSTGRES_PASSWORD=archivepw \
        -e POSTGRES_DB=archivedb -v "$REPO/init.sql:/docker-entrypoint-initdb.d/init.sql:ro" \
        -p 5543$i:5432 postgres:16 >/dev/null || fail "postgres $i"
done
# During init the server listens on the socket only, so TCP readiness means init.sql ran.
for i in 1 2; do
    for _ in $(seq 1 60); do docker exec camera-pg$i pg_isready -q -h 127.0.0.1 -U archive && break; sleep 1; done
done

tmux new-session -d -s $S -n main "sleep infinity"
run aa "$BIN/dactie-authority -K $W/aa --port $AA_PORT"
wait_count "$W/aa.log" 'Local PeerId' 1 30
AA=$(grep -o 'Local PeerId: [A-Za-z0-9]*' "$W/aa.log" | head -1 | awk '{print $3}')
echo "AA $AA"

for i in 1 2; do
    run arc$i "$BIN/dactie-archive -K $W/arc$i -u postgres://archive:archivepw@127.0.0.1:5543$i/archivedb -i archive$i -a $AA -B /ip4/127.0.0.1/tcp/$AA_PORT"
done
sleep 8
send aa end_init_archive
wait_count "$W/aa.log" 'Enough Archives registered' 1 30
wait_count "$W/arc1.log" 'Got a new ID Share' 1 30   # the AA's own share (member 0)

run p1 "$BIN/dactie-peer -K $W/p1 -b $AA -B /ip4/127.0.0.1/tcp/$AA_PORT"
sleep 4
send p1 "register_identity $AA peer1"
wait_count "$W/arc1.log" 'Got a new ID Share' 2 30
wait_count "$W/arc2.log" 'Got a new ID Share' 2 30
echo "OK join: both archives stored peer1's share"

run p2 "$BIN/dactie-peer -K $W/p2 -b $AA -B /ip4/127.0.0.1/tcp/$AA_PORT"
sleep 4
send p2 "get_opener_keys $AA"
sleep 3
send p2 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'Adding new peer from existing member' 1 30
wait_count "$W/arc1.log" 'Got a new Peer Share' 1 30
wait_count "$W/arc2.log" 'Got a new Peer Share' 1 30
echo "OK peer_from_sig: AA accepted peer2's share link"

send p1 "sub t"
send p2 "sub t"
sleep 6
send p2 "broad t camera-ready-e2e"
wait_count "$W/p1.log" 'camera-ready-e2e' 1 30
SIG=$(grep -A6 "camera-ready-e2e" "$W/p1.log" | grep -A1 '^Signature:' | tail -1 | tr -d '[:space:]')
[ -n "$SIG" ] || fail "no signature printed by peer1"
echo "OK broadcast: peer1 received peer2's message"

send aa "req_open $SIG"
for i in 1 2; do
    wait_count "$W/arc$i.log" 'Opening request [0-9a-f]* from the authority is pending' 1 30
    ID=$(grep -o 'Opening request [0-9a-f]*' "$W/arc$i.log" | tail -1 | awk '{print $3}')
    send arc$i "approve $ID"
    wait_count "$W/arc$i.log" "Submitted this archive's partial opening" 1 30
done
sleep 3
send aa "open $SIG"
wait_count "$W/aa.log" 'Signature belongs to ID' 1 60
sleep 2
grep 'Signature belongs' "$W/aa.log"
grep -q 'Name:peer1' "$W/aa.log" || fail "opening did not name peer1"
P2=$(grep -o 'Local PeerId: [A-Za-z0-9]*\|local_peer_id[^ ]* [A-Za-z0-9]*' "$W/p2.log" | head -1 | awk '{print $NF}')
echo "peer2 PeerId: ${P2:-unknown}"
if [ -n "$P2" ]; then grep -q "PeerID:.*$P2" "$W/aa.log" || fail "opening did not list peer2's PeerID"; fi
echo "PASS"
