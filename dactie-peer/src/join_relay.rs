//! Sends the messages of a join from a throwaway identity.
//!
//! The AA sees the member's name together with the PeerID a join arrives from. A join over the
//! node's own PeerID would let the AA link that PeerID, and every message it relays first, to
//! the member. The relay therefore sends VerifyIdentity, JoinKeyMaterial and the archive key
//! checks of a join from a fresh libp2p key, hands the responses to the node, and drops the key
//! once the member key has arrived. The node then registers its own PeerID as a sharing peer
//! with a slot tag, which does not reveal the member.

use dactie_utils::shared_structs::{Request, Response};
use libp2p::futures::StreamExt;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{noise, tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Addresses the relay can dial: the AA (-b, -B) and the trusted archives (--archive).
pub(crate) static ADDRESSES: OnceLock<HashMap<PeerId, Multiaddr>> = OnceLock::new();

/// Lives outside MyBehaviour because the NetworkBehaviour derive admits only behaviour fields.
#[derive(Default)]
struct State {
    outbox: VecDeque<(PeerId, Request)>,
    active: bool,
    done: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Starts a join: until finish(), its messages go through the relay.
pub(crate) fn start() {
    let mut state = state().lock().unwrap();
    state.active = true;
    state.done = false;
}

pub(crate) fn active() -> bool {
    state().lock().unwrap().active
}

pub(crate) fn send(peer: PeerId, request: Request) {
    state().lock().unwrap().outbox.push_back((peer, request));
}

/// The member key has arrived: the throwaway identity is dropped.
pub(crate) fn finish() {
    let mut state = state().lock().unwrap();
    state.active = false;
    state.done = true;
}

#[derive(NetworkBehaviour)]
pub(crate) struct RelayBehaviour {
    request_response: request_response::json::Behaviour<Request, Response>,
}

#[derive(Default)]
pub(crate) struct JoinRelay {
    swarm: Option<Swarm<RelayBehaviour>>,
}

impl JoinRelay {
    /// Sends queued requests from the throwaway identity, creating it first if needed, and
    /// drops the identity after finish().
    pub(crate) fn flush(&mut self) {
        let (queued, done) = {
            let mut state = state().lock().unwrap();
            let queued: Vec<_> = state.outbox.drain(..).collect();
            (queued, std::mem::take(&mut state.done))
        };
        if done && self.swarm.take().is_some() {
            log::info!("Dropped the throwaway identity of the join");
        }
        for (peer, request) in queued {
            let Some(addr) = ADDRESSES.get().and_then(|addresses| addresses.get(&peer)) else {
                log::error!("No address for {peer}; start the peer with -B for the authority and --archive for archives");
                continue;
            };
            let swarm = match self.swarm() {
                Ok(swarm) => swarm,
                Err(e) => {
                    log::error!("Could not create a throwaway identity for the join: {e}");
                    return;
                }
            };
            swarm.add_peer_address(peer, addr.clone());
            swarm
                .behaviour_mut()
                .request_response
                .send_request(&peer, request);
        }
    }

    fn swarm(&mut self) -> Result<&mut Swarm<RelayBehaviour>, Box<dyn std::error::Error>> {
        if self.swarm.is_none() {
            let swarm = libp2p::SwarmBuilder::with_new_identity()
                .with_tokio()
                .with_tcp(
                    tcp::Config::default(),
                    noise::Config::new,
                    yamux::Config::default,
                )?
                .with_behaviour(|_| RelayBehaviour {
                    request_response: request_response::json::Behaviour::new(
                        std::iter::once((
                            StreamProtocol::new("/key_package/1"),
                            ProtocolSupport::Outbound,
                        )),
                        request_response::Config::default(),
                    ),
                })?
                .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
                .build();
            log::info!("Joining from throwaway identity {}", swarm.local_peer_id());
            self.swarm = Some(swarm);
        }
        Ok(self.swarm.as_mut().unwrap())
    }

    /// The next response to a relayed request; pending while there is no throwaway identity.
    pub(crate) async fn next_response(&mut self) -> (PeerId, Response) {
        loop {
            let Some(swarm) = self.swarm.as_mut() else {
                return std::future::pending().await;
            };
            match swarm.select_next_some().await {
                SwarmEvent::Behaviour(RelayBehaviourEvent::RequestResponse(
                    request_response::Event::Message {
                        peer,
                        message: request_response::Message::Response { response, .. },
                    },
                )) => return (peer, response),
                SwarmEvent::Behaviour(RelayBehaviourEvent::RequestResponse(
                    request_response::Event::OutboundFailure { peer, error, .. },
                )) => log::error!("Join request to {peer} failed: {error}"),
                _ => {}
            }
        }
    }
}
