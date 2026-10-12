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
//!
//! Connected non-admitted nodes would still occupy connection slots. Since PeerIDs are free,
//! an unregistered node could fill every slot and cut a node off from members. The [`Gate`]
//! therefore keeps at most [`NON_ADMITTED_BUDGET`] non-admitted connections open at once and
//! closes each after [`GRACE`].

use crate::mls_wrapper::MyOpenMls;
use libp2p::core::Endpoint;
use libp2p::gossipsub::{self, PeerScoreParams, PeerScoreThresholds};
use libp2p::swarm::{
    dummy, CloseConnection, ConnectionClosed, ConnectionDenied, ConnectionId, FromSwarm,
    NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::{Multiaddr, PeerId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

/// Non-admitted connections a node keeps open at once.
pub const NON_ADMITTED_BUDGET: usize = 5;
/// How long a non-admitted connection may stay open: enough to register with the AA or to
/// fetch archive keys.
pub const GRACE: Duration = Duration::from_secs(60);

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

/// True for PeerIDs in the common MLS group, as last seen by [`update`].
pub fn is_admitted(peer: &PeerId) -> bool {
    SCORED
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|admitted| admitted.contains(peer))
}

/// The gate only applies once this node sees the common group, i.e. is admitted itself, so a
/// newcomer can still reach the AA and the archives.
fn gate_active() -> bool {
    SCORED
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|admitted| !admitted.is_empty())
}

#[derive(Debug)]
struct NotAdmitted;

impl std::fmt::Display for NotAdmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{NON_ADMITTED_BUDGET} non-admitted connections are already open")
    }
}

impl std::error::Error for NotAdmitted {}

/// Limits connections of nodes outside the common MLS group, see the module documentation.
#[derive(Default)]
pub struct Gate {
    /// Every open connection and when it was established.
    connections: HashMap<ConnectionId, (PeerId, Instant)>,
    to_close: VecDeque<(PeerId, ConnectionId)>,
    tick: Option<tokio::time::Interval>,
}

impl Gate {
    fn non_admitted(&self) -> impl Iterator<Item = (&ConnectionId, &(PeerId, Instant))> {
        self.connections
            .iter()
            .filter(|(_, (peer, _))| !is_admitted(peer))
    }

    fn check(&self, peer: &PeerId) -> Result<THandler<Self>, ConnectionDenied> {
        if gate_active()
            && !is_admitted(peer)
            && self.non_admitted().count() >= NON_ADMITTED_BUDGET
        {
            log::info!("Refusing connection from {peer}: not admitted");
            return Err(ConnectionDenied::new(NotAdmitted));
        }
        Ok(dummy::ConnectionHandler)
    }

    fn expired(&self, now: Instant) -> Vec<(PeerId, ConnectionId)> {
        if !gate_active() {
            return Vec::new();
        }
        self.non_admitted()
            .filter(|(_, (_, since))| now.duration_since(*since) >= GRACE)
            .map(|(id, (peer, _))| (*peer, *id))
            .collect()
    }
}

impl NetworkBehaviour for Gate {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = Infallible;

    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        peer: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.check(&peer)
    }

    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        peer: PeerId,
        _: &Multiaddr,
        _: Endpoint,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.check(&peer)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(established) => {
                self.connections.insert(
                    established.connection_id,
                    (established.peer_id, Instant::now()),
                );
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { connection_id, .. }) => {
                self.connections.remove(&connection_id);
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        _: PeerId,
        _: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {}
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        let tick = self
            .tick
            .get_or_insert_with(|| tokio::time::interval(Duration::from_secs(5)));
        let mut due = false;
        while tick.poll_tick(cx).is_ready() {
            due = true;
        }
        if due {
            for (peer, connection) in self.expired(Instant::now()) {
                if !self.to_close.contains(&(peer, connection)) {
                    log::info!("Closing connection to {peer}: not admitted within {GRACE:?}");
                    self.to_close.push_back((peer, connection));
                }
            }
        }
        match self.to_close.pop_front() {
            Some((peer_id, connection)) => Poll::Ready(ToSwarm::CloseConnection {
                peer_id,
                connection: CloseConnection::One(connection),
            }),
            None => Poll::Pending,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // One test, because the admitted set is process-wide.
    #[test]
    fn test_gate_budget_and_grace() {
        let member = PeerId::random();
        let stranger = PeerId::random();
        let mut gate = Gate::default();
        let now = Instant::now();
        let old = now.checked_sub(GRACE + Duration::from_secs(1)).unwrap();
        for _ in 0..NON_ADMITTED_BUDGET {
            gate.connections
                .insert(ConnectionId::new_unchecked(gate.connections.len()), (PeerId::random(), now));
        }

        // Before this node is admitted, the gate lets everyone in and closes nothing.
        *SCORED.lock().unwrap() = None;
        assert!(gate.check(&stranger).is_ok());
        gate.connections.insert(ConnectionId::new_unchecked(100), (stranger, old));
        assert!(gate.expired(now).is_empty());

        // Admitted: the budget is full, members still get in, the old stranger is closed.
        *SCORED.lock().unwrap() = Some(HashSet::from([member]));
        assert!(gate.check(&PeerId::random()).is_err());
        assert!(gate.check(&member).is_ok());
        gate.connections.insert(ConnectionId::new_unchecked(101), (member, old));
        assert_eq!(gate.expired(now), vec![(stranger, ConnectionId::new_unchecked(100))]);

        // A stranger that registers in time no longer counts and is not closed.
        *SCORED.lock().unwrap() = Some(HashSet::from([member, stranger]));
        assert!(gate.expired(now).is_empty());
        assert!(gate.check(&PeerId::random()).is_err()); // still 5 others open
        gate.connections.remove(&ConnectionId::new_unchecked(0));
        assert!(gate.check(&PeerId::random()).is_ok());
        *SCORED.lock().unwrap() = None;
    }
}
