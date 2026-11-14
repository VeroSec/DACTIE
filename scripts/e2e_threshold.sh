#!/bin/bash
# Local end-to-end run of threshold issuance: three AAs, any two of which issue a member key.
# The AA splits its issuing key (deal 3 2) and stays the registrar; AA 2 and AA 3 start from the
# dealt directories. A member joins with all three AAs up, a second one with AA 3 down, and a
# third one with AA 2 down as well, which must fail (one partial credential is not enough).
# The members' messages are signed and opened as with a single AA. Finally AA 2 and AA 3 issue
# a fourth member's key while the registrar is down.
# Run with the setup in e2e_common.sh, which also describes the requirements. About 4 minutes.
source "$(dirname "$0")/e2e_common.sh"
AA2_PORT=$((AA_PORT + 2))
AA3_PORT=$((AA_PORT + 3))

start_network --threshold 2
send aa "deal 3 2 $W/issuers"
wait_count "$W/aa.log" 'Dealt 3 key shares' 1 20
for i in 1 2; do
    wait_count "$W/arc$i.log" 'Accepting ID shares from the AAs' 1 20
done
ARCHIVES=""
for i in 1 2; do
    addr=$(grep -o '/ip4/127.0.0.1/tcp/[0-9]*' "$W/arc$i.log" | head -1)
    ARCHIVES="$ARCHIVES --archive $addr/p2p/$(peer_id arc$i)"
done
run aa2 "$BIN/dactie-authority -K $W/issuers/aa2 --port $AA2_PORT $ARCHIVES"
run aa3 "$BIN/dactie-authority -K $W/issuers/aa3 --port $AA3_PORT $ARCHIVES"
wait_count "$W/aa2.log" 'Local PeerId' 1 30
wait_count "$W/aa3.log" 'Local PeerId' 1 30
AA2=$(peer_id aa2)
AA3=$(peer_id aa3)
grep -q "AA 2: PeerId $AA2" "$W/aa.log" || fail "AA 2 does not run the dealt identity"
PEER_ARGS="--issuer /ip4/127.0.0.1/tcp/$AA2_PORT/p2p/$AA2 --issuer /ip4/127.0.0.1/tcp/$AA3_PORT/p2p/$AA3"
echo "OK deal: AA 2 ($AA2) and AA 3 ($AA3) run with their key shares"

join_member p1 peer1
grep -q 'credential from several AAs' "$W/p1.log" || fail "p1 did not combine partial credentials"
echo "OK join with 3 AAs: $(grep -c 'Partial credential' "$W/p1.log") partial credentials arrived, 2 were combined"

tmux kill-window -t "$S:aa3"
join_member p2 peer2
grep -q 'credential from several AAs' "$W/p2.log" || fail "p2 did not combine partial credentials"
echo "OK join with AA 3 down: the AA and AA 2 issued peer2's key"

tmux kill-window -t "$S:aa2"
start_peer p3
sleep 4
send p3 "register_identity $AA peer3"
sleep 15
grep -q 'credential from several AAs' "$W/p3.log" && fail "p3 got a member key from a single AA"
grep -q 'Partial credential 1 of 2' "$W/p3.log" || fail "p3 got no partial credential from the AA"
echo "OK join with only the AA up: one partial credential, no member key"

send p1 "sub t"
send p2 "sub t"
sleep 6
send p2 "broad t threshold-broadcast"
wait_count "$W/p1.log" 'threshold-broadcast' 1 30
SIG=$(grep -A6 "threshold-broadcast" "$W/p1.log" | grep -A1 '^Signature:' | tail -1 | tr -d '[:space:]')
echo "OK broadcast: p2's message, signed with the combined key, reached p1"
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
grep -q 'Name:peer2' "$W/aa.log" || fail "the opening did not name peer2"
echo "OK opening: the archives opened p2's message to peer2"

# Any two of three: with the registrar down, AA 2 and AA 3 issue a member key on their own.
# Registering the node as a sharing peer then waits for the registrar.
run aa2 "$BIN/dactie-authority -K $W/issuers/aa2 --port $AA2_PORT $ARCHIVES"
run aa3 "$BIN/dactie-authority -K $W/issuers/aa3 --port $AA3_PORT $ARCHIVES"
wait_count "$W/aa2.log" 'Local PeerId' 1 30
wait_count "$W/aa3.log" 'Local PeerId' 1 30
tmux kill-window -t "$S:aa"
start_peer p4
sleep 4
send p4 "register_identity $AA peer4"
wait_count "$W/p4.log" 'credential from several AAs' 1 30
grep -q 'Partial credential 2 of 2 from AA [23]' "$W/p4.log" || fail "p4's key did not come from AA 2 and AA 3"
echo "OK join with the registrar down: AA 2 and AA 3 issued peer4's key"
echo PASS
