//! Connection cap that refuses before the handshake.
//!
//! libp2p's `connection_limits` checks established connections only after the
//! security handshake. A full peer therefore still pays for every Noise
//! handshake it then rejects, including the ones its own Kademlia lookups dial,
//! and each rejected dial drops the target from the routing table, which later
//! triggers yet another bootstrap. go-libp2p's resource manager refuses when a
//! connection opens instead; this behaviour does the same. A peer holding `max`
//! connections refuses to dial and refuses to upgrade accepted sockets. The
//! checks after the handshake only catch connections that were already pending
//! when the peer filled up.

use libp2p::core::{Endpoint, Multiaddr};
use libp2p::swarm::{
    behaviour::ConnectionEstablished, dummy, ConnectionClosed, ConnectionDenied, ConnectionId,
    FromSwarm, NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::PeerId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::task::{Context, Poll};

pub(crate) struct ConnCap {
    max: usize,
    established: HashSet<ConnectionId>,
    per_peer: HashMap<PeerId, usize>,
}

#[derive(Debug)]
pub(crate) enum Refused {
    Full(usize),
    AlreadyConnected,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Full(max) => write!(f, "connection cap of {max} reached"),
            Refused::AlreadyConnected => write!(f, "already connected to this peer"),
        }
    }
}

impl std::error::Error for Refused {}

impl ConnCap {
    /// `None` leaves the peer uncapped (the bootstrap peer).
    pub(crate) fn new(max: Option<usize>) -> Self {
        ConnCap {
            max: max.unwrap_or(usize::MAX),
            established: HashSet::new(),
            per_peer: HashMap::new(),
        }
    }

    fn check_room(&self) -> Result<(), ConnectionDenied> {
        if self.established.len() >= self.max {
            return Err(ConnectionDenied::new(Refused::Full(self.max)));
        }
        Ok(())
    }

    fn check_peer(&self, peer: &PeerId) -> Result<(), ConnectionDenied> {
        if self.per_peer.contains_key(peer) {
            return Err(ConnectionDenied::new(Refused::AlreadyConnected));
        }
        Ok(())
    }
}

impl NetworkBehaviour for ConnCap {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = std::convert::Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        _: ConnectionId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.check_room()
    }

    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        peer: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.check_room()?;
        self.check_peer(&peer)?;
        Ok(dummy::ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _: ConnectionId,
        peer: Option<PeerId>,
        _: &[Multiaddr],
        _: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.check_room()?;
        if let Some(peer) = peer {
            self.check_peer(&peer)?;
        }
        Ok(vec![])
    }

    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        peer: PeerId,
        _: &Multiaddr,
        _: Endpoint,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.check_room()?;
        self.check_peer(&peer)?;
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                ..
            }) => {
                if self.established.insert(connection_id) {
                    *self.per_peer.entry(peer_id).or_default() += 1;
                }
            }
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                connection_id,
                ..
            }) => {
                if self.established.remove(&connection_id) {
                    if let Some(n) = self.per_peer.get_mut(&peer_id) {
                        *n -= 1;
                        if *n == 0 {
                            self.per_peer.remove(&peer_id);
                        }
                    }
                }
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

    fn poll(&mut self, _: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::swarm::ConnectionId;

    fn established(cap: &mut ConnCap, peer: PeerId, id: ConnectionId) {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1".parse().unwrap();
        cap.handle_established_inbound_connection(id, peer, &addr, &addr)
            .expect("room left");
        cap.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: peer,
            connection_id: id,
            endpoint: &libp2p::core::ConnectedPoint::Listener {
                local_addr: addr.clone(),
                send_back_addr: addr.clone(),
            },
            failed_addresses: &[],
            other_established: 0,
        }));
    }

    /// A full peer refuses before dialing and before upgrading, and frees the
    /// slot again once a connection closes.
    #[test]
    fn refuses_when_full_before_the_handshake() {
        let mut cap = ConnCap::new(Some(2));
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1".parse().unwrap();
        let (a, b) = (PeerId::random(), PeerId::random());
        established(&mut cap, a, ConnectionId::new_unchecked(1));
        // A second connection to the same peer is refused even with room left.
        assert!(cap
            .handle_pending_outbound_connection(ConnectionId::new_unchecked(9), Some(a), &[], Endpoint::Dialer)
            .is_err());
        established(&mut cap, b, ConnectionId::new_unchecked(2));
        assert!(cap
            .handle_pending_inbound_connection(ConnectionId::new_unchecked(3), &addr, &addr)
            .is_err());
        assert!(cap
            .handle_pending_outbound_connection(ConnectionId::new_unchecked(4), None, &[], Endpoint::Dialer)
            .is_err());
        cap.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id: a,
            connection_id: ConnectionId::new_unchecked(1),
            endpoint: &libp2p::core::ConnectedPoint::Listener {
                local_addr: addr.clone(),
                send_back_addr: addr.clone(),
            },
            remaining_established: 0,
        }));
        assert!(cap
            .handle_pending_inbound_connection(ConnectionId::new_unchecked(5), &addr, &addr)
            .is_ok());
    }

    #[test]
    fn uncapped_without_a_limit() {
        let mut cap = ConnCap::new(None);
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1".parse().unwrap();
        assert!(cap
            .handle_pending_inbound_connection(ConnectionId::new_unchecked(1), &addr, &addr)
            .is_ok());
    }
}
