//! Restricts GossipSub to admitted nodes, i.e. the PeerIDs in the common MLS group.
//!
//! Any libp2p node can connect to a DACTIE node, e.g. to register with the AA or to fetch
//! archive keys, and GossipSub itself has no access control. A node that never registered
//! could otherwise join meshes and see which neighbor relays which message first. Nodes that
//! are not in the common group get an application score below GossipSub's publish threshold:
//! they are never grafted into a mesh, are pruned at every heartbeat, and receive neither our
//! messages nor gossip. The score stays above the graylist threshold, so their subscriptions
//! are still recorded and a newly admitted peer joins meshes as soon as the commit that adds
//! it arrives. Like members, they still see which channels a node subscribes to.

use crate::mls_wrapper::MyOpenMls;
use libp2p::gossipsub::{self, PeerScoreParams, PeerScoreThresholds};
use libp2p::PeerId;
use std::collections::HashSet;
use std::sync::Mutex;

/// Below the default publish threshold (-50), above the default graylist threshold (-80).
const NOT_ADMITTED_SCORE: f64 = -60.0;

/// The admitted PeerIDs the scores were last set for. One node per process.
static SCORED: Mutex<Option<HashSet<PeerId>>> = Mutex::new(None);

/// Enables peer scoring in which only admission counts.
pub fn enable(gossipsub: &mut gossipsub::Behaviour) -> Result<(), String> {
    let params = PeerScoreParams {
        app_specific_weight: 1.0,
        // Nodes may share an IP (NAT, or one host in tests), and behaviour penalties
        // could push members out of meshes; neither should decide mesh membership.
        ip_colocation_factor_weight: 0.0,
        behaviour_penalty_weight: 0.0,
        ..Default::default()
    };
    gossipsub.with_peer_score(params, PeerScoreThresholds::default())
}

/// Call after every swarm event and instruction. Re-scores all connected peers when the
/// common group changed, and scores a newly connected peer.
pub fn update(
    gossipsub: &mut gossipsub::Behaviour,
    my_open_mls: &MyOpenMls,
    connected: Option<&PeerId>,
) {
    let admitted = my_open_mls.common_group_peers();
    let mut scored = SCORED.lock().unwrap();
    if scored.as_ref() != Some(&admitted) {
        let peers: Vec<PeerId> = gossipsub.all_peers().map(|(peer, _)| *peer).collect();
        for peer in &peers {
            score(gossipsub, &admitted, peer);
        }
        log::info!("Admitted {} PeerIDs to GossipSub", admitted.len());
        *scored = Some(admitted);
    } else if let Some(peer) = connected {
        score(gossipsub, &admitted, peer);
    }
}

fn score(gossipsub: &mut gossipsub::Behaviour, admitted: &HashSet<PeerId>, peer: &PeerId) {
    let score = if admitted.contains(peer) {
        0.0
    } else {
        NOT_ADMITTED_SCORE
    };
    gossipsub.set_application_score(peer, score);
}
