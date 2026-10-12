//! Connection cap for DACTIE nodes that refuses before the handshake.
//!
//! libp2p's `connection_limits` checks established connections only after the
//! Noise handshake, so a full node pays for every handshake it then rejects.
//! go-libp2p's resource manager refuses when a connection opens instead, and so
//! does this behaviour:
//!
//! - A node holding `max_established` connections refuses to dial and refuses
//!   to upgrade accepted sockets.
//! - Inbound connections take at most `max_inbound` of those slots, so a node
//!   whose inbound side is saturated can still dial the authority, the archives
//!   and admitted peers.
//! - At most `max_pending_inbound` accepted sockets may be mid-handshake. Before
//!   the handshake a node cannot tell who connects, so pending sockets do not
//!   count against the established cap, where they could lock admitted peers
//!   out; this separate limit keeps sockets that never finish the handshake
//!   from exhausting the node, and libp2p's upgrade timeout drops them.
//! - One connection per peer.
//! - Exempt peers (the authority, trusted archives) bypass the caps wherever
//!   their PeerId is known: when the node dials them and after the handshake.
//!   An exempt peer that dials a full node is still refused before the
//!   handshake, so nodes keep their own connections to these peers, which
//!   trimming never closes.
//!
//! The anonymity experiment has its own, simpler copy without exemptions or
//! pending limit (`dactie-experiment-anonymity/src/conn_cap.rs`).

use libp2p::core::{ConnectedPoint, Endpoint, Multiaddr};
use libp2p::swarm::{
    behaviour::{ConnectionEstablished, ListenFailure},
    dummy, ConnectionClosed, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour,
    THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::PeerId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::task::{Context, Poll};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_established: usize,
    pub max_inbound: usize,
    pub max_pending_inbound: usize,
}

impl Limits {
    /// Limits of a node that keeps at most `high` connections: inbound may take
    /// 80% of them, and 32 sockets may be mid-handshake at once.
    pub fn for_high_watermark(high: usize) -> Self {
        let high = high.max(1);
        Limits {
            max_established: high,
            max_inbound: (high * 4 / 5).max(1),
            max_pending_inbound: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Full(usize),
    InboundFull(usize),
    PendingInboundFull(usize),
    AlreadyConnected,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Full(n) => write!(f, "connection cap of {n} reached"),
            Refused::InboundFull(n) => write!(f, "inbound cap of {n} reached"),
            Refused::PendingInboundFull(n) => write!(f, "{n} inbound handshakes already pending"),
            Refused::AlreadyConnected => write!(f, "already connected to this peer"),
        }
    }
}

impl std::error::Error for Refused {}

pub struct ConnCap {
    limits: Limits,
    exempt: HashSet<PeerId>,
    inbound: HashSet<ConnectionId>,
    outbound: HashSet<ConnectionId>,
    pending_inbound: HashSet<ConnectionId>,
    per_peer: HashMap<PeerId, usize>,
}

impl ConnCap {
    pub fn new(limits: Limits, exempt: impl IntoIterator<Item = PeerId>) -> Self {
        ConnCap {
            limits,
            exempt: exempt.into_iter().collect(),
            inbound: HashSet::new(),
            outbound: HashSet::new(),
            pending_inbound: HashSet::new(),
            per_peer: HashMap::new(),
        }
    }

    fn established(&self) -> usize {
        self.inbound.len() + self.outbound.len()
    }

    fn deny(reason: Refused) -> Result<(), ConnectionDenied> {
        Err(ConnectionDenied::new(reason))
    }

    fn check_total(&self) -> Result<(), ConnectionDenied> {
        if self.established() >= self.limits.max_established {
            return Self::deny(Refused::Full(self.limits.max_established));
        }
        Ok(())
    }

    fn check_inbound(&self) -> Result<(), ConnectionDenied> {
        self.check_total()?;
        if self.inbound.len() >= self.limits.max_inbound {
            return Self::deny(Refused::InboundFull(self.limits.max_inbound));
        }
        Ok(())
    }

    fn check_peer(&self, peer: &PeerId) -> Result<(), ConnectionDenied> {
        if self.per_peer.contains_key(peer) {
            return Self::deny(Refused::AlreadyConnected);
        }
        Ok(())
    }
}

impl NetworkBehaviour for ConnCap {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = std::convert::Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        if self.pending_inbound.len() >= self.limits.max_pending_inbound {
            return Self::deny(Refused::PendingInboundFull(self.limits.max_pending_inbound));
        }
        self.check_inbound()?;
        self.pending_inbound.insert(connection_id);
        Ok(())
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.pending_inbound.remove(&connection_id);
        if !self.exempt.contains(&peer) {
            self.check_peer(&peer)?;
            self.check_inbound()?;
        }
        Ok(dummy::ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _: ConnectionId,
        peer: Option<PeerId>,
        _: &[Multiaddr],
        _: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        match peer {
            Some(peer) if self.exempt.contains(&peer) => {}
            Some(peer) => {
                self.check_peer(&peer)?;
                self.check_total()?;
            }
            None => self.check_total()?,
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
        if !self.exempt.contains(&peer) {
            self.check_peer(&peer)?;
            self.check_total()?;
        }
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                ..
            }) => {
                let new = match endpoint {
                    ConnectedPoint::Listener { .. } => self.inbound.insert(connection_id),
                    ConnectedPoint::Dialer { .. } => self.outbound.insert(connection_id),
                };
                if new {
                    *self.per_peer.entry(peer_id).or_default() += 1;
                }
            }
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                connection_id,
                ..
            }) => {
                let known =
                    self.inbound.remove(&connection_id) | self.outbound.remove(&connection_id);
                if known {
                    if let Some(n) = self.per_peer.get_mut(&peer_id) {
                        *n -= 1;
                        if *n == 0 {
                            self.per_peer.remove(&peer_id);
                        }
                    }
                }
            }
            FromSwarm::ListenFailure(ListenFailure { connection_id, .. }) => {
                self.pending_inbound.remove(&connection_id);
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

    fn addr() -> Multiaddr {
        "/ip4/127.0.0.1/tcp/1".parse().unwrap()
    }

    fn listener() -> ConnectedPoint {
        ConnectedPoint::Listener {
            local_addr: addr(),
            send_back_addr: addr(),
        }
    }

    fn dialer() -> ConnectedPoint {
        ConnectedPoint::Dialer {
            address: addr(),
            role_override: Endpoint::Dialer,
        }
    }

    fn id(n: usize) -> ConnectionId {
        ConnectionId::new_unchecked(n)
    }

    /// Accept an inbound socket, finish its handshake and record it.
    fn inbound(cap: &mut ConnCap, n: usize, peer: PeerId) -> Result<(), ConnectionDenied> {
        cap.handle_pending_inbound_connection(id(n), &addr(), &addr())?;
        cap.handle_established_inbound_connection(id(n), peer, &addr(), &addr())?;
        let endpoint = listener();
        cap.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: peer,
            connection_id: id(n),
            endpoint: &endpoint,
            failed_addresses: &[],
            other_established: 0,
        }));
        Ok(())
    }

    fn outbound(cap: &mut ConnCap, n: usize, peer: PeerId) -> Result<(), ConnectionDenied> {
        cap.handle_pending_outbound_connection(id(n), Some(peer), &[], Endpoint::Dialer)?;
        cap.handle_established_outbound_connection(id(n), peer, &addr(), Endpoint::Dialer)?;
        let endpoint = dialer();
        cap.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: peer,
            connection_id: id(n),
            endpoint: &endpoint,
            failed_addresses: &[],
            other_established: 0,
        }));
        Ok(())
    }

    fn limits(max_established: usize, max_inbound: usize, max_pending_inbound: usize) -> Limits {
        Limits {
            max_established,
            max_inbound,
            max_pending_inbound,
        }
    }

    #[test]
    fn default_limits_reserve_a_fifth_for_dialing() {
        assert_eq!(Limits::for_high_watermark(100), limits(100, 80, 32));
        assert_eq!(Limits::for_high_watermark(1), limits(1, 1, 32));
    }

    /// With the inbound side full, the node still dials until the total cap.
    #[test]
    fn saturated_inbound_leaves_slots_for_dialing() {
        let mut cap = ConnCap::new(limits(4, 2, 8), []);
        inbound(&mut cap, 1, PeerId::random()).unwrap();
        inbound(&mut cap, 2, PeerId::random()).unwrap();
        assert!(cap.handle_pending_inbound_connection(id(3), &addr(), &addr()).is_err());
        outbound(&mut cap, 4, PeerId::random()).unwrap();
        outbound(&mut cap, 5, PeerId::random()).unwrap();
        assert!(cap
            .handle_pending_outbound_connection(id(6), Some(PeerId::random()), &[], Endpoint::Dialer)
            .is_err());
    }

    /// Sockets stuck in the handshake fill only the pending limit; once one
    /// fails, the next socket is accepted, and they never use established slots.
    #[test]
    fn stalled_handshakes_fill_only_the_pending_limit() {
        let mut cap = ConnCap::new(limits(4, 4, 2), []);
        cap.handle_pending_inbound_connection(id(1), &addr(), &addr()).unwrap();
        cap.handle_pending_inbound_connection(id(2), &addr(), &addr()).unwrap();
        assert!(cap.handle_pending_inbound_connection(id(3), &addr(), &addr()).is_err());
        outbound(&mut cap, 4, PeerId::random()).unwrap();
        let (local, remote) = (addr(), addr());
        cap.on_swarm_event(FromSwarm::ListenFailure(ListenFailure {
            local_addr: &local,
            send_back_addr: &remote,
            error: &libp2p::swarm::ListenError::Aborted,
            connection_id: id(1),
            peer_id: None,
        }));
        assert!(cap.handle_pending_inbound_connection(id(5), &addr(), &addr()).is_ok());
    }

    /// The authority and trusted archives get through a full node whenever
    /// their PeerId is known, and their connections free no other peer's slot.
    #[test]
    fn exempt_peers_bypass_the_caps() {
        let authority = PeerId::random();
        let mut cap = ConnCap::new(limits(1, 1, 8), [authority]);
        inbound(&mut cap, 1, PeerId::random()).unwrap();
        outbound(&mut cap, 2, authority).unwrap();
        assert!(cap
            .handle_established_inbound_connection(id(3), authority, &addr(), &addr())
            .is_ok());
        assert!(cap
            .handle_pending_outbound_connection(id(4), Some(PeerId::random()), &[], Endpoint::Dialer)
            .is_err());
    }

    #[test]
    fn one_connection_per_peer_and_slots_return_on_close() {
        let peer = PeerId::random();
        let mut cap = ConnCap::new(limits(2, 2, 8), []);
        outbound(&mut cap, 1, peer).unwrap();
        assert!(cap
            .handle_pending_outbound_connection(id(2), Some(peer), &[], Endpoint::Dialer)
            .is_err());
        let endpoint = dialer();
        cap.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id: peer,
            connection_id: id(1),
            endpoint: &endpoint,
            remaining_established: 0,
        }));
        outbound(&mut cap, 3, peer).unwrap();
    }
}
