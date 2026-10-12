#!/bin/bash
# Local end-to-end run: join, sharing peers with slot tags, a broadcast, and an opening.
# AA, two archives (PostgreSQL in Docker), member peer1, and peer2 as a second sharing peer
# of the same member (peer_from_sig) with slot 1 of the member's k = 2 slots; the AA refuses a
# slot twice, and a member cannot register more than k. peer2 publishes, both archives approve
# the opening,
# and the AA must name peer1's member and list peer2's PeerID. An observer node that never
# registered subscribes to the same topic and must not receive any gossip; the AA keeps at
# most 5 such connections open and closes each after 60 s.
# Run with the setup in e2e_common.sh, which also describes the requirements.
source "$(dirname "$0")/e2e_common.sh"

start_network --peers-per-member 2
join_member p1 peer1
echo "OK join: both archives stored peer1's share"

start_peer p2
sleep 4
send p2 "get_opener_keys $AA"
sleep 3
send p2 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'Adding new peer from existing member' 1 30
wait_count "$W/arc1.log" 'Got a new Peer Share' 1 30
wait_count "$W/arc2.log" 'Got a new Peer Share' 1 30
echo "OK peer_from_sig: AA accepted peer2's shares and slot tag"

# The member forgets which slots it used and tries slot 1 again for a third peer.
rm "$W/p1/slots.json"
start_peer p3
sleep 4
send p3 "get_opener_keys $AA"
sleep 3
send p3 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'slot 1 of epoch [0-9]* was already used by this member' 1 30
wait_count "$W/p3.log" 'Authority refused this peer' 1 30
[ "$(grep -c 'Adding new peer from existing member' "$W/aa.log")" -eq 1 ] || fail "the AA admitted a peer with a used slot"
echo "OK slot reuse: the AA refused slot 1 a second time"

# Slot 2 is free, and then the member has used both of its k = 2 slots.
send p3 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'Adding new peer from existing member' 2 30
start_peer p5
sleep 4
send p5 "get_opener_keys $AA"
sleep 3
send p5 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/p5.log" 'All slots of epoch [0-9]* are used by this member' 1 30
echo "OK cap: slot 2 registered peer3, a third sharing peer found no free slot"

# An unregistered node, connected like any other, subscribed to the same channel.
start_peer obs "info,libp2p_swarm=debug"
OBS_START=$(date +%s)
sleep 4
send obs "sub t"
send p1 "sub t"
send p2 "sub t"
sleep 6
send p2 "broad t camera-ready-e2e"
wait_count "$W/p1.log" 'camera-ready-e2e' 1 30
SIG=$(grep -A6 "camera-ready-e2e" "$W/p1.log" | grep -A1 '^Signature:' | tail -1 | tr -d '[:space:]')
[ -n "$SIG" ] || fail "no signature printed by peer1"
echo "OK broadcast: peer1 received peer2's message"
sleep 3
grep -q "Connection established peer=$AA" "$W/obs.log" \
    || fail "observer never connected to the AA, so the check below proves nothing"
if grep -q 'Received gossipsub payload' "$W/obs.log"; then
    fail "an observer that never registered received gossip"
fi
echo "OK observer: an unregistered node received no gossip"

# Six more unregistered nodes: with the observer that is 7, over the AA's budget of 5.
for i in 2 3 4 5 6 7; do
    start_peer obs$i
done
wait_count "$W/aa.log" 'Refusing connection from .*: not admitted' 2 30
echo "OK budget: the AA refused unregistered connections beyond 5"

OBS=$(peer_id obs)
left=$((OBS_START + 75 - $(date +%s)))
[ "$left" -gt 0 ] && sleep "$left"
grep -q "Closing connection to $OBS: not admitted" "$W/aa.log" || fail "the AA kept the observer connected past the grace period"
echo "OK grace: the AA closed the observer's connection after 60 s"

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
P2=$(peer_id p2)
echo "peer2 PeerId: ${P2:-unknown}"
if [ -n "$P2" ]; then grep -q "PeerID:.*$P2" "$W/aa.log" || fail "opening did not list peer2's PeerID"; fi
echo "PASS"
