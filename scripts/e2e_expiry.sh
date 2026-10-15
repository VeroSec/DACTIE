#!/bin/bash
# Local end-to-end run of epoch expiry, with 60-second epochs and k = 4.
# Within one epoch, member peer1 registers sharing peers p2, p3 and p4, and p2 and p3 renew with
# slots of the next epoch. After the epoch ends, the AA must remove p4 from the common group
# and keep p2 and p3, and p4 must receive no gossip while p2's messages reach p3. peer1's own
# registration from the join may expire as well; it plays no part here.
# Run with the setup in e2e_common.sh, which also describes the requirements. About 3 minutes.
source "$(dirname "$0")/e2e_common.sh"
EPOCH=60

start_network --epoch-secs $EPOCH --peers-per-member 4
join_member p1 peer1
echo "OK join"

# Start just after an epoch boundary, so that the setup fits into one epoch.
while [ $(( $(date +%s) % EPOCH )) -gt 1 ]; do sleep 1; done
E=$(( $(date +%s) / EPOCH ))
add_sharing_peer p2 p1
add_sharing_peer p3 p1
add_sharing_peer p4 p1
send p2 "renew $AA $W/p1/key_material.json"
send p3 "renew $AA $W/p1/key_material.json"
wait_count "$W/aa.log" "Renewed sharing peer .* until epoch $((E + 1))" 2 30
[ $(( $(date +%s) / EPOCH )) -eq "$E" ] || fail "the setup crossed an epoch boundary; run again"
echo "OK renew: p2 and p3 are registered until epoch $((E + 1)), p4 until epoch $E"

# The AA checks for expired peers every 5 s.
P2=$(peer_id p2)
P3=$(peer_id p3)
P4=$(peer_id p4)
wait_count "$W/aa.log" "Expired .*$P4" 1 $((EPOCH + 15))
grep 'Expired' "$W/aa.log" | grep -q "$P2\|$P3" && fail "the AA expired a renewed peer"
echo "OK expiry: the AA removed p4 from the common group and kept p2 and p3"

send p2 "sub t"
send p3 "sub t"
send p4 "sub t"
sleep 6
before=$(grep -c 'Received gossipsub payload' "$W/p4.log")
send p2 "broad t after-expiry"
wait_count "$W/p3.log" 'after-expiry' 1 30
sleep 3
[ "$(grep -c 'Received gossipsub payload' "$W/p4.log")" -eq "$before" ] || fail "the expired p4 still received gossip"
echo "OK gossip: p3 got p2's message, the expired p4 got nothing"
# p3 usually ends p2's stem and delivers the message itself: it must show the stem predecessor
# as the propagating peer, never itself, and the sender as anonymous.
SHOWN=$(grep -A6 'after-expiry' "$W/p3.log")
echo "$SHOWN" | grep -q "Propagating Peer: $P3" && fail "p3 showed itself as the propagating peer"
echo "$SHOWN" | grep -q 'Sender: anonymous' || fail "p3 did not show p2's message as anonymous"
echo "OK stem: p3 showed $(echo "$SHOWN" | grep -o 'Propagating Peer: [A-Za-z0-9]*' | awk '{print $3}' | sed "s/$P2/p2/") as the propagating peer, sender anonymous"
echo "PASS"
