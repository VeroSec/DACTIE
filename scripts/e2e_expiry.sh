#!/bin/bash
# Local end-to-end run of epoch expiry, with 60-second epochs and k = 2.
# Within one epoch, member peer1 registers sharing peers p2 and p4 and renews p2 with a token
# for the next epoch. After the epoch ends, the AA must remove p4 from the common group and
# keep p2, and p4 must receive no gossip while p2's messages still reach peer1.
# Run with the setup in e2e_common.sh, which also describes the requirements. About 3 minutes.
source "$(dirname "$0")/e2e_common.sh"
EPOCH=60

start_network --epoch-secs $EPOCH --peers-per-member 2
join_member p1 peer1
echo "OK join"

# Start just after an epoch boundary, so that the setup fits into one epoch.
while [ $(( $(date +%s) % EPOCH )) -gt 1 ]; do sleep 1; done
E=$(( $(date +%s) / EPOCH ))
send p1 "get_tokens $AA 2"
wait_count "$W/p1.log" "Received 2 registration tokens for epoch $E" 1 30
send p1 "get_tokens $AA 1 next"
wait_count "$W/p1.log" "Received 1 registration tokens for epoch $((E + 1))" 1 30
echo "OK tokens: 2 for epoch $E, 1 for epoch $((E + 1))"

add_sharing_peer p2 p1
add_sharing_peer p4 p1
send p2 "renew $AA $W/p1/key_material.json"
wait_count "$W/aa.log" "Renewed sharing peer .* until epoch $((E + 1))" 1 30
[ $(( $(date +%s) / EPOCH )) -eq "$E" ] || fail "the setup crossed an epoch boundary; run again"
echo "OK renew: p2 is registered until epoch $((E + 1)), p4 until epoch $E"

# The AA checks for expired peers every 5 s.
wait_count "$W/aa.log" 'Expired 1 sharing peers' 1 $((EPOCH + 15))
P2=$(peer_id p2)
P4=$(peer_id p4)
grep 'Expired 1 sharing peers' "$W/aa.log" | grep -q "$P4" || fail "the AA did not expire p4"
grep 'Expired' "$W/aa.log" | grep -q "$P2" && fail "the AA expired the renewed p2"
echo "OK expiry: the AA removed p4 from the common group and kept p2"

send p1 "sub t"
send p2 "sub t"
send p4 "sub t"
sleep 6
before=$(grep -c 'Received gossipsub payload' "$W/p4.log")
send p2 "broad t after-expiry"
wait_count "$W/p1.log" 'after-expiry' 1 30
sleep 3
[ "$(grep -c 'Received gossipsub payload' "$W/p4.log")" -eq "$before" ] || fail "the expired p4 still received gossip"
echo "OK gossip: peer1 got p2's message, the expired p4 got nothing"
echo "PASS"
