#!/bin/bash
# Local end-to-end run of private groups nested inside the common group, with 120-second epochs
# and k = 4. Within one epoch, member peer1 registers sharing peers p2, p3 and p4; p2 and p3
# renew, p4 does not. p2 creates a private group with p3 and p4, and the three read each other's
# messages in it. Once the AA has expired p4, p2 must remove p4 from the private group, p3 must
# still read p2's private messages, and nothing p4 sends there may be delivered.
# That a removed peer can neither send nor read in a private group it still belongs to is shown
# by the unit tests in crates/dactie-utils/src/mls_wrapper.rs (nested_group_tests): after the expiry,
# the admission gate usually closes p4's connections, so its messages need not arrive at all.
# Run with the setup in e2e_common.sh, which also describes the requirements. About 5 minutes.
source "$(dirname "$0")/e2e_common.sh"
EPOCH=120

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
P2=$(peer_id p2)
P3=$(peer_id p3)
P4=$(peer_id p4)

send p2 "req_kps $P3 $P4"
wait_count "$W/p2.log" 'Response with KeyPackage' 2 30
send p2 "create_group $P3 $P4"
wait_count "$W/p3.log" 'Added Group' 1 30
wait_count "$W/p4.log" 'Added Group' 1 30
sleep 3 # the new members subscribe to the group's channel
send p2 "enc_broad 0 from-p2-before"
wait_count "$W/p3.log" 'from-p2-before' 1 30
wait_count "$W/p4.log" 'from-p2-before' 1 30
send p4 "enc_broad 0 from-p4-before"
wait_count "$W/p2.log" 'from-p4-before' 1 30
wait_count "$W/p3.log" 'from-p4-before' 1 30
[ $(( $(date +%s) / EPOCH )) -eq "$E" ] || fail "the setup crossed an epoch boundary; run again"
echo "OK private group: p2, p3 and p4 read each other's messages"

# The AA checks for expired peers every 5 s.
wait_count "$W/aa.log" "Expired .*$P4" 1 $((EPOCH + 15))
grep 'Expired' "$W/aa.log" | grep -q "$P2\|$P3" && fail "the AA expired a renewed peer"
echo "OK expiry: the AA removed p4 from the common group"

wait_count "$W/p2.log" "Removed $P4 from private group 0" 1 30
echo "OK nested: p2 removed p4 from its private group"

sleep 3 # p3 applies p2's removal commit
send p2 "enc_broad 0 from-p2-after"
wait_count "$W/p3.log" 'from-p2-after' 1 30
send p4 "enc_broad 0 from-p4-after"
sleep 10
grep -q 'from-p2-after' "$W/p4.log" && fail "the removed p4 read p2's private message"
grep -q 'from-p4-after' "$W/p2.log" "$W/p3.log" && fail "a private message of the removed p4 was delivered"
DROPPED=$(cat "$W/p2.log" "$W/p3.log" | grep -c 'Dropping a private-group message')
echo "OK after removal: p3 read p2's message, p4 read nothing, and nothing p4 sent was delivered ($DROPPED of its messages arrived and were dropped)"
echo "PASS"
