#!/bin/bash
# Shared setup for the local end-to-end runs (e2e_local.sh, e2e_expiry.sh).
# Needs Docker, tmux and `cargo build --release`. Logs go to target/e2e/.
# Leaves tmux session "camera" and containers camera-pg1/2 running for inspection;
# clean up with: tmux kill-session -t camera; docker rm -f camera-pg1 camera-pg2
# To run next to another run, set E2E_SESSION (tmux session and container prefix),
# E2E_AA_PORT and E2E_PG_PORT (PostgreSQL uses E2E_PG_PORT+1 and +2), and W (logs).
set -uo pipefail
REPO=${REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
BIN=$REPO/target/release
W=${W:-$REPO/target/e2e}
S=${E2E_SESSION:-camera}
AA_PORT=${E2E_AA_PORT:-10001}
PG_PORT=${E2E_PG_PORT:-55430}

fail() { echo "FAIL: $*"; exit 1; }
wait_count() { # file pattern count seconds
    for _ in $(seq 1 "$4"); do
        n=$(grep -c -- "$2" "$1" 2>/dev/null)
        [ "${n:-0}" -ge "$3" ] && return 0
        sleep 1
    done
    fail "waited $4 s for $3x '$2' in $(basename "$1")"
}
run() { tmux new-window -d -t $S -n "$1" "cd $W && RUST_LOG=${3:-info} $2 2>&1 | tee $W/$1.log"; }
send() { tmux send-keys -t "$S:$1" "$2" Enter; }
peer_id() { grep -o 'Local PeerId: [A-Za-z0-9]*' "$W/$1.log" | head -1 | awk '{print $3}'; }

# Starts PostgreSQL, the AA (with extra arguments), two archives, and ends the init phase.
start_network() {
    rm -rf "$W"; mkdir -p "$W"
    tmux kill-session -t $S 2>/dev/null
    docker rm -f $S-pg1 $S-pg2 >/dev/null 2>&1
    for i in 1 2; do
        docker run -d --rm --name $S-pg$i -e POSTGRES_USER=archive -e POSTGRES_PASSWORD=archivepw \
            -e POSTGRES_DB=archivedb -v "$REPO/init.sql:/docker-entrypoint-initdb.d/init.sql:ro" \
            -p $((PG_PORT + i)):5432 postgres:16 >/dev/null || fail "postgres $i"
    done
    # During init the server listens on the socket only, so TCP readiness means init.sql ran.
    for i in 1 2; do
        for _ in $(seq 1 60); do docker exec $S-pg$i pg_isready -q -h 127.0.0.1 -U archive && break; sleep 1; done
    done

    tmux new-session -d -s $S -n main "sleep infinity"
    mkdir -p "$W"/{aa,arc1,arc2}
    run aa "$BIN/dactie-authority -K $W/aa --port $AA_PORT $*"
    wait_count "$W/aa.log" 'Local PeerId' 1 30
    AA=$(peer_id aa)
    echo "AA $AA"
    for i in 1 2; do
        run arc$i "$BIN/dactie-archive -K $W/arc$i -u postgres://archive:archivepw@127.0.0.1:$((PG_PORT + i))/archivedb -i archive$i -a $AA -B /ip4/127.0.0.1/tcp/$AA_PORT"
    done
    sleep 8
    send aa end_init_archive
    wait_count "$W/aa.log" 'Enough Archives registered' 1 30
    wait_count "$W/arc1.log" 'Got a new ID Share' 1 30   # the AA's own share (member 0)
}

# Starts a peer node.
start_peer() { # window [RUST_LOG]
    mkdir -p "$W/$1"
    run "$1" "$BIN/dactie-peer -K $W/$1 -b $AA -B /ip4/127.0.0.1/tcp/$AA_PORT" "${2:-info}"
}

# Registers a member from a new peer node. The node joins from a throwaway identity and then
# registers its own PeerID as a sharing peer.
join_member() { # window name
    local shares added
    shares=$(grep -c 'Got a new ID Share' "$W/arc1.log")
    added=$(grep -c 'Adding new peer from existing member' "$W/aa.log")
    start_peer "$1"
    sleep 4
    send "$1" "register_identity $AA $2"
    wait_count "$W/arc1.log" 'Got a new ID Share' $((shares + 1)) 30
    wait_count "$W/arc2.log" 'Got a new ID Share' $((shares + 1)) 30
    wait_count "$W/aa.log" 'Adding new peer from existing member' $((added + 1)) 30
}

# Starts a peer node and registers it as a sharing peer of the member in window $2.
add_sharing_peer() { # window member_window
    local added
    added=$(grep -c 'Adding new peer from existing member' "$W/aa.log")
    start_peer "$1"
    sleep 4
    send "$1" "get_opener_keys $AA"
    sleep 3
    send "$1" "peer_from_sig $AA $W/$2/key_material.json"
    wait_count "$W/aa.log" 'Adding new peer from existing member' $((added + 1)) 30
}
