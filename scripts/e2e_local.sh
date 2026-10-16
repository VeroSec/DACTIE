#!/bin/bash
# Local end-to-end run: join, sharing peers with slot tags, an anonymous and an attributed
# broadcast, and an opening.
# AA, two archives (PostgreSQL in Docker), and member peer1, which joins from a throwaway
# identity and then registers its own PeerID with slot 1 of the member's k = 3 slots. peer2 is a
# second sharing peer of the same member (peer_from_sig, slot 2); the AA refuses a slot twice,
# and a member cannot register more than k. peer2 publishes, both archives approve the opening,
# and the AA must name peer1's member and list peer2's PeerID. An observer node that never
# registered subscribes to the same topic and must not receive any gossip; the AA keeps at
# most 5 such connections open and closes each after 60 s. A second member's peer then publishes
# a broadcast through the stem of member 1's peers, and its opening must name member 2.
# Run with the setup in e2e_common.sh, which also describes the requirements.
source "$(dirname "$0")/e2e_common.sh"

start_network --peers-per-member 3
join_member p1 peer1
P1=$(peer_id p1)
JOINED_FROM=$(grep -o 'Verified identity peer1 from [A-Za-z0-9]*' "$W/aa.log" | awk '{print $NF}')
[ -n "$JOINED_FROM" ] && [ "$JOINED_FROM" != "$P1" ] || fail "the AA saw peer1's own PeerID at the join"
grep -q "Joining from throwaway identity $JOINED_FROM" "$W/p1.log" || fail "the join did not come from peer1's throwaway identity"
echo "OK join: the AA linked the name only to throwaway $JOINED_FROM; peer1 then registered its own PeerID"

# Counted from here: peer1's own registration is the first.
add_sharing_peer p2 p1
grep -q 'Registering with slot 2 ' "$W/p2.log" || fail "peer2 did not take slot 2"
wait_count "$W/arc1.log" 'Got a new Peer Share' 2 30
wait_count "$W/arc2.log" 'Got a new Peer Share' 2 30
echo "OK peer_from_sig: AA accepted peer2's shares and slot tag (slot 2)"

# The member forgets which slots it used and tries slots 1 and 2 again for a third peer.
rm "$W/p1/slots.json"
start_peer p3
sleep 4
send p3 "get_opener_keys $AA"
sleep 3
send p3 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'slot 1 of epoch [0-9]* was already used by this member' 1 30
wait_count "$W/p3.log" 'Authority refused this peer' 1 30
send p3 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'slot 2 of epoch [0-9]* was already used by this member' 1 30
wait_count "$W/p3.log" 'Authority refused this peer' 2 30
[ "$(grep -c 'Adding new peer from existing member' "$W/aa.log")" -eq 2 ] || fail "the AA admitted a peer with a used slot"
echo "OK slot reuse: the AA refused slots 1 and 2 a second time"

# Slot 3 is free, and then the member has used all of its k = 3 slots.
send p3 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/aa.log" 'Adding new peer from existing member' 3 30
start_peer p5
sleep 4
send p5 "get_opener_keys $AA"
sleep 3
send p5 "peer_from_sig $AA $W/p1/key_material.json"
wait_count "$W/p5.log" 'All slots of epoch [0-9]* are used by this member' 1 30
echo "OK cap: slot 3 registered peer3, a fourth sharing peer found no free slot"

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
grep -A6 "camera-ready-e2e" "$W/p1.log" | grep -q 'Sender: anonymous' || fail "the broadcast was not shown as anonymous"
echo "OK broadcast: peer1 received peer2's message, sender anonymous"

# Only the channel travels in clear; the topic is inside the ciphertext. "other2"
# shares channel_9 with "t" (channel_of in mls_wrapper.rs), so peer1 receives and
# decrypts it, but must not show it: it subscribed to "t" only.
send p2 "broad other2 same-channel-e2e"
sleep 2
send p2 "broad t after-other-e2e"
wait_count "$W/p1.log" 'after-other-e2e' 1 30
sleep 2
if grep -q 'same-channel-e2e' "$W/p1.log"; then fail "peer1 showed a broadcast on a topic it did not subscribe to"; fi
ROW=$(docker exec $S-pg1 psql -U archive -d archivedb -tAc "SELECT channel, topic FROM broadcast WHERE data = convert_to('same-channel-e2e', 'UTF8')")
[ "$ROW" = "channel_9|other2" ] || fail "archive1 stored '$ROW' instead of channel_9|other2"
echo "OK topic inside the ciphertext: peer1 dropped another topic of its channel, archive1 stored channel and topic"

# Stem phase (Dandelion++): anonymous broadcasts first travel along a stem of
# admitted sharing peers; only a diffuser publishes them into GossipSub.
grep -q 'Stem: sent own message to' "$W/p2.log" || fail "peer2's anonymous broadcasts did not enter the stem"
STEMMED=$(grep -c 'Stem: sent own message to' "$W/p2.log")
echo "OK stem: peer2's anonymous broadcasts entered the stem ($STEMMED times)"

# Attributed: the signature names peer1's member, whose MemberID only the AA logged.
MID=$(grep -o 'Created Key Material for member [0-9]*' "$W/aa.log" | head -1 | awk '{print $NF}')
send p2 "attr_broad t attributed-e2e"
wait_count "$W/p1.log" 'attributed-e2e' 1 30
grep -A6 "attributed-e2e" "$W/p1.log" | grep -q "Attributed to member: $MID" || fail "the attributed broadcast did not name member $MID"
echo "OK attribution: peer1 saw the attributed message as from member $MID"
[ "$(grep -c 'Stem: sent own message to' "$W/p2.log")" = "$STEMMED" ] || fail "the attributed broadcast went through the stem"
echo "OK stem: the attributed broadcast was published directly"

# A second member whose sharing peer m2 is the source. Its stem can only run through member 1's
# peers, so one of them publishes the message; the opening at the end must still name member 2.
join_member m2 member2
M2=$(peer_id m2)
send m2 "sub t"
sleep 10
send m2 "broad t member2-e2e"
wait_count "$W/p1.log" 'member2-e2e' 1 60
grep -q 'Stem: sent own message to' "$W/m2.log" || fail "member 2's broadcast did not enter the stem"
SIG2=$(grep -A6 "member2-e2e" "$W/p1.log" | grep -A1 '^Signature:' | tail -1 | tr -d '[:space:]')
echo "OK stem: member 2's broadcast went into the stem of member 1's peers"
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

# Over a minute after the broadcasts. peer3 does not subscribe to t's channel, so it never sees
# a stem message arrive in GossipSub and must not arm an embargo (it would publish late copies).
FORWARDED=$(grep -c 'Stem: relayed a message' "$W/p3.log")
grep -q 'Stem: embargo ran out' "$W/p3.log" && fail "peer3, outside the channel, published after an embargo"
echo "OK stem: peer3, outside the channel, forwarded $FORWARDED stem messages and armed no embargo"
# A peer that ends a stem shows its stem predecessor as the propagating peer, never itself.
for n in p1 p2 p3; do
    grep -q "Propagating Peer: $(peer_id $n)" "$W/$n.log" && fail "$n showed itself as the propagating peer"
done
echo "OK stem: no peer showed itself as the propagating peer"
# A peer that publishes a stem message itself must cancel its own embargo; otherwise the timer
# fires later and the peer shows (and after 60 s re-sends) the message a second time.
for n in p1 p2 p3; do
    TWICE=$(grep -A1 'Got Broadcastmessage' "$W/$n.log" | grep -v -e 'Got Broadcastmessage' -e '^--' | sort | uniq -d)
    [ -z "$TWICE" ] || fail "$n showed a broadcast twice: $TWICE"
done
echo "OK stem: no peer showed a broadcast twice"

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

# Member 2's message, published by one of member 1's peers: no stem hop may be taken for the author.
grep -q 'Stem: embargo ran out' "$W/m2.log" && fail "m2 published its own message after the embargo, so no stem hop did"
send aa "req_open $SIG2"
for i in 1 2; do
    wait_count "$W/arc$i.log" 'Opening request [0-9a-f]* from the authority is pending' 2 30
    ID=$(grep -o 'Opening request [0-9a-f]*' "$W/arc$i.log" | tail -1 | awk '{print $3}')
    send arc$i "approve $ID"
    wait_count "$W/arc$i.log" "Submitted this archive's partial opening" 2 30
done
sleep 3
send aa "open $SIG2"
wait_count "$W/aa.log" 'Signature belongs to ID' 2 60
sleep 2
OPENED=$(awk '/Signature belongs to ID/{n++} n==2' "$W/aa.log" | grep 'Signature belongs')
echo "$OPENED" | grep -q 'Name:member2' || fail "the opening of member 2's message did not name member2"
echo "$OPENED" | grep -q "PeerID:.*$M2" || fail "the opening of member 2's message did not list m2's PeerID"
echo "$OPENED" | grep -q 'Name:peer1' && fail "the opening named member 1, whose peers carried the stem"
echo "OK authorship: member 1's peers published member 2's message; the opening named member2 and m2"
echo "PASS"
