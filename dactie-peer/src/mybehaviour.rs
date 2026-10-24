use crate::mybehaviour::MyBehaviourError::PubSubBehaviourError;
use async_trait::async_trait;
use dactie_utils::net::{is_unroutable_addr, MESH_N, MESH_N_HIGH, MESH_N_LOW};
use crate::join_relay;
use crate::outstanding::{self, Asked};
use dactie_utils::admission;
use dactie_utils::conn_cap;
use dactie_utils::stem::{self, Resend, Route, StemAck, StemCodec, StemConfig, StemMessage, STEM_PROTOCOL};
use crate::stem_relay;
use dactie_utils::key_store;
use dactie_utils::slot_store::SlotStore;
use libp2p::gossipsub::{IdentTopic, MessageAcceptance};
use libp2p::identity::{Keypair, ParseError};
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{Mode, PROTOCOL_NAME};
use libp2p::request_response::{OutboundRequestId, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identify, kad, request_response, Multiaddr, PeerId, StreamProtocol};
use openmls::key_packages::KeyPackage;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io;
use tokio::sync::mpsc;

use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::instruction_handler::InstructionHandler;
use crate::abstswarm::AbstSwarmError::{
    HandleUpdateError, MLSError, MessageValidationError, SendError, SendJoinGroupError,
    SendKeyPackageError,
};
use crate::abstswarm::{AbstSwarmError, Notification};
use dactie_utils::mls_wrapper::MyOpenMLSError::{
    GroupNotAvailable, KeyMaterialFileError, SerializationError,
};
use dactie_utils::mls_wrapper::{MyOpenMLSError, MyOpenMls};
use dactie_utils::shared_structs::*;

/// Archive keys handed out by the authority, held until each archive confirms its own key.
///
/// Lives outside MyBehaviour for the same reason as `archive_query_timing`.
pub(crate) mod archive_keys {
    use libp2p::PeerId;
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    /// Archives this peer trusts, configured out of band (`--archive`). Empty: no verification.
    pub(crate) static TRUSTED: OnceLock<Vec<PeerId>> = OnceLock::new();

    /// The join waits here while the archives confirm their keys.
    pub(crate) struct PendingJoin {
        pub(crate) authority: PeerId,
        pub(crate) id: usize,
        pub(crate) n: thressig::G1,
        pub(crate) group_key: Vec<u8>,
    }

    pub(crate) struct Pending {
        pub(crate) keys: Vec<(PeerId, thressig::G2)>,
        pub(crate) threshold: usize,
        pub(crate) confirmed: HashSet<PeerId>,
        pub(crate) join: Option<PendingJoin>,
    }

    pub(crate) fn pending() -> &'static Mutex<Option<Pending>> {
        static P: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
        P.get_or_init(|| Mutex::new(None))
    }

    pub(crate) fn trusted() -> &'static [PeerId] {
        TRUSTED.get().map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Outstanding archive queries, keyed by request id, for round-trip timing.
///
/// This lives outside MyBehaviour because the NetworkBehaviour derive admits only
/// behaviour fields. Bench builds only: a normal peer does not measure itself.
#[cfg(feature = "bench")]
pub(crate) mod archive_query_timing {
    use libp2p::request_response::OutboundRequestId;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    fn table() -> &'static Mutex<HashMap<OutboundRequestId, Instant>> {
        static T: OnceLock<Mutex<HashMap<OutboundRequestId, Instant>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn started(id: OutboundRequestId) {
        table().lock().unwrap().insert(id, Instant::now());
    }

    /// Consumes the entry: a response is only measured once.
    pub(crate) fn finished(id: &OutboundRequestId) -> Option<std::time::Duration> {
        table().lock().unwrap().remove(id).map(|t| t.elapsed())
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "MyBehaviourEvent")]
pub(crate) struct MyBehaviour {
    /// The Gossipsub pub/sub behaviour is used to send broadcast messages to peers.
    pub(crate) gossipsub: gossipsub::Behaviour,
    /// Send more detailed identifying info to connected peers, a.o the listen_address.
    /// This address can then be used to populate the Kademlia DHT.
    pub(crate) identify: identify::Behaviour,
    /// The Kademlia DHT used to discover peers
    pub(crate) kademlia: kad::Behaviour<MemoryStore>,
    /// In this case, RequestResponse behaviour is used to send direct messages to peers,
    /// this is used for the creation of MLS Groups
    pub(crate) request_response: request_response::json::Behaviour<Request, Response>,
    /// Limits connections of nodes outside the common MLS group.
    pub(crate) gate: admission::Gate,
    /// Caps connections at the high watermark, refusing before the handshake
    pub(crate) limits: conn_cap::ConnCap,
    /// Stem hops of the Dandelion++ phase, one request per hop
    pub(crate) stem: request_response::Behaviour<StemCodec>,
}

impl crate::abstswarm::StemTick for MyBehaviour {
    /// Publishes stem messages whose embargo ran out without them showing up
    /// in GossipSub; a relay also delivers them here.
    fn stem_tick(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
    ) {
        let now = Instant::now();
        let due = {
            let mut state = stem_relay::state();
            state.expire(now);
            state.due(now)
        };
        for (message, from) in due {
            log::info!("Stem: embargo ran out, publishing");
            let result = match from {
                None => {
                    stem_relay::state().in_gossip(stem::message_id(&message.payload));
                    self.publish_to(&message.channel, message.payload)
                }
                Some(from) => self.publish_relayed(notification_tx, my_open_mls, message, from),
            };
            if let Err(e) = result {
                log::warn!("Stem: could not publish after the embargo: {e}");
            }
        }
    }
}

impl crate::abstswarm::ProtectedPeers for MyBehaviour {
    fn protected_peers(&self) -> std::collections::HashSet<PeerId> {
        let mut peers: std::collections::HashSet<PeerId> =
            self.gossipsub.all_mesh_peers().copied().collect();
        peers.extend(archive_keys::trusted().iter().copied());
        peers
    }
}

impl MyBehaviour {
    pub(crate) fn new(
        keypair: &Keypair,
        boot_peer_id_option: Option<PeerId>,
        boot_peer_addr: Multiaddr,
        archives: &[(PeerId, Multiaddr)],
        conn_high: usize,
        stem_config: StemConfig,
    ) -> Result<MyBehaviour, MyBehaviourError> {
        // To content-address message, we can take the hash of message and use it as an ID.
        let message_id_fn = |message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        };

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(1)) // This is set to aid debugging by not cluttering the log space
            .validation_mode(gossipsub::ValidationMode::Permissive) // This sets the kind of message validation. The default is Strict (enforce message signing)
            .validate_messages() // Allows validation of group signatures
            .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
            .flood_publish(false) //Flood Pub Deactivate to provide Source Anonymity
            .mesh_n_low(MESH_N_LOW)
            .mesh_n(MESH_N)
            .mesh_n_high(MESH_N_HIGH)
            .fanout_ttl(admission::FANOUT_TTL)
            // Real CTI events are far larger than gossipsub's 65536-byte default,
            // which a 64KiB MISP event already exceeds once the group signature
            // and framing are added. Raised so the evaluation can carry actual
            // MISP payloads; every peer must agree on this or large events are
            // dropped on receipt.
            .max_transmit_size(8 * 1024 * 1024)
            .build()
            .map_err(io::Error::other)
            .unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.

        let mut gossipsub =
            gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Anonymous, gossipsub_config)
                .map_err(|err| PubSubBehaviourError(err.to_string()))?;
        admission::enable(&mut gossipsub).map_err(PubSubBehaviourError)?;

        gossipsub
            .subscribe(&IdentTopic::new("Test"))
            .map_err(|err| PubSubBehaviourError(err.to_string()))?;

        let identify = identify::Behaviour::new(identify::Config::new(
            identify::PROTOCOL_NAME.to_string(),
            keypair.public(),
        ));

        let peer_id = keypair.public().to_peer_id();

        let mut cfg = kad::Config::new(PROTOCOL_NAME);
        cfg.set_query_timeout(Duration::from_secs(5 * 60));
        let store = MemoryStore::new(peer_id);
        let mut kademlia = kad::Behaviour::with_config(peer_id, store, cfg);

        kademlia.set_mode(Some(Mode::Server));

        if let Some(boot_peer_id) = boot_peer_id_option {
            // Add the bootnodes to the local routing table.
            kademlia.add_address(&boot_peer_id, boot_peer_addr);
        }
        // Trusted archives are dialed directly to confirm their keys.
        for (archive, addr) in archives {
            kademlia.add_address(archive, addr.clone());
        }

        let request_response = request_response::json::Behaviour::new(
            iter::once((StreamProtocol::new("/key_package/1"), ProtocolSupport::Full)),
            request_response::Config::default(),
        );

        // Refuses before the handshake once full. The authority and the trusted
        // archives are exempt, like they are from trimming.
        let limits = conn_cap::ConnCap::new(
            conn_cap::Limits::for_high_watermark(conn_high),
            boot_peer_id_option
                .into_iter()
                .chain(archives.iter().map(|(archive, _)| *archive)),
        );

        stem_relay::init(stem_config);
        let stem = request_response::Behaviour::<StemCodec>::new(
            iter::once((STEM_PROTOCOL, ProtocolSupport::Full)),
            request_response::Config::default(),
        );

        Ok(Self {
            gossipsub,
            identify,
            kademlia,
            request_response,
            gate: admission::Gate::default(),
            limits,
            stem,
        })
    }

    fn notify(notification_tx: &mpsc::UnboundedSender<Notification>, notification: Notification) {
        if let Err(e) = notification_tx.send(notification) {
            log::error!("Failed to send notification back to router through mpsc channel: {e}");
        }
    }

    pub fn notify_error(
        notification_tx: &mpsc::UnboundedSender<Notification>,
        error: AbstSwarmError,
    ) {
        if let Err(e) = notification_tx.send(Notification::Err(error)) {
            log::error!("Failed to send notification back to router through mpsc channel: {e}");
        }
    }

    /// Sends a direct request and records what it asks for, see `outstanding`.
    fn send_request(&mut self, peer_id: &PeerId, request: Request) -> OutboundRequestId {
        let asked = Asked::of(&request.body);
        let request_id = self.request_response.send_request(peer_id, request);
        outstanding::sent(request_id, asked);
        request_id
    }

    //Subscribe to Gossipsub topic
    fn subscribe(&mut self, topic: Vec<u8>, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        // Use partitioned Channels to hide Subscriptions
        match my_open_mls.subscribe(topic.clone()) {
            Ok(opt_channel) => {
                if let Some(channel) = opt_channel {
                    log::debug!("Subscribing to channel {channel:?}");
                    if let Err(error) = self.gossipsub.subscribe(&IdentTopic::new(channel.clone()))
                    {
                        log::error!("Failed to subscribe to channel '{}': {}", channel, error,);
                        return Err(AbstSwarmError::SubscriptionError {
                            reason: format!("{error}"),
                        });
                    }
                }
            }
            Err(err) => return Err(MLSError(err)),
        }
        my_open_mls.store_to_file()?;
        Ok(())
    }

    //Unsubscribe from Gossipsub topic
    fn unsubscribe(
        &mut self,
        topic: Vec<u8>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        match my_open_mls.unsubscribe(topic) {
            Ok(opt_channel) => {
                if let Some(channel) = opt_channel {
                    log::debug!("Unsubscribing from channel {channel:?}");
                    if let Err(error) = self
                        .gossipsub
                        .unsubscribe(&IdentTopic::new(channel.clone()))
                    {
                        log::error!(
                            "Failed to unsubscribe from channel '{}': {}",
                            channel,
                            error,
                        );
                        return Err(AbstSwarmError::SubscriptionError {
                            reason: format!("{error}"),
                        });
                    }
                }
            }
            Err(err) => return Err(MLSError(err)),
        }
        my_open_mls.store_to_file()?;

        Ok(())
    }

    // Send a message over gossipsub
    fn send(&mut self, body: MessageBody, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        self.send_signed(body, my_open_mls, false)
    }

    /// Encrypts and publishes a message; `attributed` signs it so that receivers see this
    /// member's MemberID.
    fn send_signed(
        &mut self,
        body: MessageBody,
        my_open_mls: &MyOpenMls,
        attributed: bool,
    ) -> Result<(), AbstSwarmError> {
        // Anonymous broadcasts enter the stem. Attributed messages name their
        // member anyway, and MLS control messages are not CTI.
        let stem_eligible = !attributed
            && matches!(body, MessageBody::Broadcast { .. } | MessageBody::Group { .. })
            && stem_relay::state().is_on();
        let (body, channel) = match body.clone() {
            MessageBody::Broadcast { ref topic, data } => (
                self.send_to_topic(topic, data, my_open_mls)?,
                my_open_mls.get_channel(topic.as_bytes().to_vec()),
            ),
            MessageBody::Group { group_number, data } => {
                let (body, group_id) = self.send_to_group(my_open_mls, group_number, data)?;
                (body, my_open_mls.get_channel(group_id))
            }
            MessageBody::Commit { group_id, .. } => (body, my_open_mls.get_channel(group_id)),
            MessageBody::Proposal { group_id, .. } => (body, my_open_mls.get_channel(group_id)),
            _ => {
                return Err(SendError {
                    reason: "Wrong Message Body for sending".to_string(),
                })
            }
        };

        let message = if attributed {
            my_open_mls.sign_attributed(body)?
        } else {
            my_open_mls.sign(body)?
        };

        let to_send =
            bincode::serialize(&message).map_err(|e| SerializationError(e.to_string()))?;
        if stem_eligible {
            return self.stem_publish_own(channel, to_send);
        }
        self.publish_to(&channel, to_send)
    }

    /// Publishes a signed gossip message on a channel.
    fn publish_to(&mut self, channel: &str, payload: Vec<u8>) -> Result<(), AbstSwarmError> {
        log::info!("Publishing gossipsub payload: {} bytes", payload.len());
        if let Err(error) = self
            .gossipsub
            .publish(IdentTopic::new(channel.to_owned()), payload)
        {
            match error {
                gossipsub::PublishError::InsufficientPeers => {
                    log::debug!("Could not publish, no peers subscribed to channel")
                }
                gossipsub::PublishError::Duplicate => {
                    log::debug!("Not publishing: the message is already out")
                }
                e => {
                    return Err(SendError {
                        reason: format!("{e}"),
                    })
                }
            }
        }
        Ok(())
    }

    /// Connected, admitted sharing peers that speak the stem protocol; of those,
    /// the ones this peer dialled itself if there are at least two.
    fn stem_candidates(&self) -> Vec<PeerId> {
        let candidates: Vec<PeerId> = {
            let capable = stem_relay::capable();
            self.gossipsub
                .all_peers()
                .map(|(peer, _)| *peer)
                .filter(|peer| capable.contains(peer) && admission::is_admitted(peer))
                .collect()
        };
        stem::prefer_outbound(candidates, &stem_relay::outbound())
    }

    /// Sends one of this peer's own messages into the stem.
    fn stem_publish_own(&mut self, channel: String, payload: Vec<u8>) -> Result<(), AbstSwarmError> {
        let id = stem::message_id(&payload);
        let candidates = self.stem_candidates();
        let route = {
            let mut state = stem_relay::state();
            state.first_time(id);
            state.mark_own(id);
            state.route(None, &candidates, Instant::now(), &mut rand::thread_rng())
        };
        match route {
            Route::Forward(next) => {
                log::info!("Stem: sent own message {id:016x} to {next}");
                self.send_stem(next, id, StemMessage { channel, payload }, None);
                Ok(())
            }
            Route::Fluff => {
                log::info!("Stem: no stem peer available, publishing directly");
                self.publish_to(&channel, payload)
            }
        }
    }

    /// Hands a message to the next stem hop; `from` is its stem predecessor,
    /// `None` for this peer's own. Only a peer that subscribes to the message's
    /// channel arms the embargo: any other never sees the message in GossipSub,
    /// so its timer would always run out and publish a late copy, which GossipSub
    /// may deliver twice and which marks this peer as a stem member.
    fn send_stem(&mut self, next: PeerId, id: u64, message: StemMessage, from: Option<PeerId>) {
        let topic = IdentTopic::new(message.channel.clone()).hash();
        let subscribed = self.gossipsub.topics().any(|t| *t == topic);
        {
            let mut state = stem_relay::state();
            if from.is_none() {
                state.sent_own(id, next);
            }
            if subscribed {
                state.arm_embargo(id, message.clone(), from, Instant::now(), &mut rand::thread_rng());
            }
        }
        let request = self.stem.send_request(&next, message.clone());
        stem_relay::pending().insert(request, (message, from));
    }

    /// Re-routes this peer's own message after it came back from `came_back_from`
    /// or its hop to `failed` failed, see `StemState::retry_own`. Returns false if
    /// the message is not its own.
    fn retry_own_stem(
        &mut self,
        id: u64,
        message: &StemMessage,
        came_back_from: Option<PeerId>,
        failed: Option<PeerId>,
    ) -> bool {
        if !stem_relay::state().is_own(id) {
            return false;
        }
        let candidates = self.stem_candidates();
        let next = stem_relay::state().retry_own(
            id,
            came_back_from,
            failed,
            &candidates,
            Instant::now(),
            &mut rand::thread_rng(),
        );
        match next {
            Some(Resend::Fresh(next)) => {
                log::info!("Stem: re-routing own message {id:016x} to {next}");
                self.send_stem(next, id, message.clone(), None);
            }
            Some(Resend::FirstHop(next)) => {
                log::info!("Stem: every stem peer has own message {id:016x}, back to the first hop {next}");
                self.send_stem(next, id, message.clone(), None);
            }
            None => log::info!("Stem: own message left to the embargo"),
        }
        true
    }

    /// Publishes a stem message that another peer originated, and delivers it
    /// here too: GossipSub never hands a peer its own publication back, so a
    /// diffuser that is also a recipient would otherwise never see the message.
    /// The client sees `from`, the stem predecessor, as the propagating peer, as
    /// it sees the forwarding neighbour for gossip; the author is only ever
    /// what the group signature says.
    fn publish_relayed(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        message: StemMessage,
        from: PeerId,
    ) -> Result<(), AbstSwarmError> {
        let payload = message.payload.clone();
        {
            let mut state = stem_relay::state();
            let id = stem::message_id(&payload);
            if state.is_in_gossip(id) {
                // Out already, and shown here when it arrived or was published.
                return Ok(());
            }
            state.in_gossip(id);
        }
        self.publish_to(&message.channel, message.payload)?;
        let Ok(gossip) = bincode::deserialize::<Message>(&payload) else {
            return Ok(());
        };
        let Ok((body, signature, attributed_to)) = my_open_mls.verify_message(gossip) else {
            return Ok(());
        };
        self.deliver(notification_tx, my_open_mls, body, signature, attributed_to, from)
    }

    /// A stem message arrived from `from`.
    fn on_stem(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        from: PeerId,
        message: StemMessage,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        if !admission::is_admitted(&from) {
            log::debug!("Stem: ignoring a message from non-admitted {from}");
            return Ok(());
        }
        let allowed = stem_relay::state().allow(from, Instant::now());
        if !allowed {
            log::warn!("Stem: {from} exceeds the stem rate limit, dropping");
            return Ok(());
        }
        let id = stem::message_id(&message.payload);
        if stem_relay::state().is_in_gossip(id) {
            // The message is out already: a relay's embargo ran out early, or a hop
            // arrived although its acknowledgement was lost. The stem is done.
            log::info!("Stem: dropping a message from {from} that is already in GossipSub");
            return Ok(());
        }
        // Forward only what the network would accept: a broadcast with a valid
        // group signature, for the channel it names.
        let gossip: Message = match bincode::deserialize(&message.payload) {
            Ok(gossip) => gossip,
            Err(e) => {
                log::warn!("Stem: undecodable message from {from}: {e}");
                return Ok(());
            }
        };
        let expected = match &gossip.body {
            MessageBody::BroadcastEnc { channel, .. } => channel.clone(),
            MessageBody::GroupEnc { group_id, .. } => my_open_mls.get_channel(group_id.clone()),
            _ => {
                log::warn!("Stem: {from} sent something other than a broadcast");
                return Ok(());
            }
        };
        if expected != message.channel {
            log::warn!("Stem: {from} sent a message for {expected} as {}", message.channel);
            return Ok(());
        }
        if let Err(e) = my_open_mls.verify_message(gossip) {
            log::warn!("Stem: invalid message from {from}: {e}");
            return Ok(());
        }
        let first = stem_relay::state().first_time(id);
        if !first {
            // The stem looped back here. A relay publishes the message now, which
            // reveals only the relay. The source never does: it sends the message
            // to a peer that does not have it yet (its other successor, else a
            // spare connection), which sees an ordinary new stem message. A peer
            // that got the same message twice from it would know it is the source.
            if !self.retry_own_stem(id, &message, Some(from), None) {
                log::info!("Stem: loop at this relay, publishing a message from {from}");
                self.publish_relayed(notification_tx, my_open_mls, message, from)?;
            }
            return Ok(());
        }
        let candidates = self.stem_candidates();
        let route = stem_relay::state().route(Some(from), &candidates, Instant::now(), &mut rand::thread_rng());
        match route {
            Route::Forward(next) => {
                log::info!("Stem: relayed a message from {from} to {next}");
                self.send_stem(next, id, message, Some(from));
                Ok(())
            }
            Route::Fluff => {
                log::info!("Stem: diffusing a message from {from}");
                self.publish_relayed(notification_tx, my_open_mls, message, from)
            }
        }
    }

    fn handle_stem_event(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        event: request_response::Event<StemMessage, StemAck>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        match event {
            request_response::Event::Message { peer, message } => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let _ = self.stem.send_response(channel, StemAck);
                    self.on_stem(notification_tx, peer, request, my_open_mls)
                }
                request_response::Message::Response { request_id, .. } => {
                    stem_relay::pending().remove(&request_id);
                    Ok(())
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
            } => {
                let failed = stem_relay::pending().remove(&request_id);
                if let Some((message, from)) = failed {
                    log::info!("Stem: hop to {peer} failed: {error}");
                    let id = stem::message_id(&message.payload);
                    match from {
                        None => {
                            self.retry_own_stem(id, &message, None, Some(peer));
                        }
                        Some(from) => self.publish_relayed(notification_tx, my_open_mls, message, from)?,
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn send_to_topic(
        &mut self,
        topic: &str,
        message: Vec<u8>,
        my_open_mls: &MyOpenMls,
    ) -> Result<MessageBody, AbstSwarmError> {
        log::debug!("Broadcasting: {:?}", message.clone());

        // The topic goes inside the ciphertext; only its channel stays in clear.
        Ok(my_open_mls.seal_broadcast(topic, message)?)
    }

    fn send_to_group(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_number: usize,
        data: Vec<u8>,
    ) -> Result<(MessageBody, Vec<u8>), AbstSwarmError> {
        let group_id = my_open_mls.get_group_id(group_number)?;
        let (group_id, ciphertext, nonce) = my_open_mls.encrypt_with_group_key(group_id, data)?;
        Ok((
            MessageBody::GroupEnc {
                group_id: group_id.clone(),
                enc_data: ciphertext,
                nonce,
            },
            group_id,
        ))
    }

    // Get key Pakcage from other Peers
    fn get_keypackages(
        &mut self,
        my_open_mls: &MyOpenMls,
        peer_ids: Vec<PeerId>,
    ) -> Result<(), AbstSwarmError> {
        for peer_id in peer_ids {
            let request: Request = my_open_mls.sign(RequestMessage::KeyPackage)?;
            self.send_request(&peer_id, request);
        }
        Ok(())
    }

    // Create new MLS Group
    fn create_group(
        &mut self,
        peer_ids: Vec<PeerId>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        let group_number = my_open_mls.create_mls_group(&peer_ids)?;
        let group_id = my_open_mls.get_group_id(group_number)?;
        //Subscribes to partitionedchannel and adds groupid to topic list
        self.subscribe(group_id.to_vec(), my_open_mls)?;

        Ok(())
    }

    // List all MLS groups
    fn list_groups(&mut self, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        for (index, group_id) in my_open_mls.get_groups().iter().enumerate() {
            println!("[{index}]: {group_id:?}");
        }
        Ok(())
    }

    // Add new Group Member to MLS Group
    fn add_group_member(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_number: usize,
        peer_id: PeerId,
    ) -> Result<(), AbstSwarmError> {
        let (group_id, mls_message_out, epoch) =
            my_open_mls.propose_add_group_member(group_number, peer_id)?;
        self.send_proposal(my_open_mls, group_id, mls_message_out, epoch)
    }

    // Remove Group Member from MLS Group
    fn remove_group_member(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_number: usize,
        peer_id: PeerId,
    ) -> Result<(), AbstSwarmError> {
        let (group_id, mls_message_out, epoch) =
            my_open_mls.propose_remove_group_member(group_number, peer_id)?;
        self.send_proposal(my_open_mls, group_id, mls_message_out, epoch)
    }

    // Deleve Group for all members(including owner)
    fn delete_group_for_all(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_number: usize,
    ) -> Result<(), AbstSwarmError> {
        let group_id = my_open_mls.delete_group_for_all(group_number)?;
        my_open_mls.remove_mls_group(group_id.clone());
        self.unsubscribe(group_id, my_open_mls)
    }

    // Send a new group proposal
    fn send_proposal(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_id: Vec<u8>,
        data: Vec<u8>,
        epoch: u64,
    ) -> Result<(), AbstSwarmError> {
        let body = MessageBody::Proposal {
            group_id: group_id.clone(),
            data,
            epoch,
        };
        self.send(body, my_open_mls)
    }

    // Update Key material for group
    fn update_key_material(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_number: usize,
    ) -> Result<(), AbstSwarmError> {
        let (group_id, mls_message_out, epoch) = my_open_mls.update_key_material(group_number)?;
        self.send_proposal(my_open_mls, group_id, mls_message_out, epoch)
    }

    // Request Group Updates from an Archive Peer
    fn update_groups_req(
        &mut self,
        my_open_mls: &MyOpenMls,
        peer_id: PeerId,
    ) -> Result<(), AbstSwarmError> {
        for group_id in my_open_mls.get_groups() {
            let epoch = my_open_mls
                .get_group_epoch(&group_id)
                .ok_or(GroupNotAvailable(
                    "Could not find group mentioned in update req".to_string(),
                ))?;
            let _req_id = self.send_request(
                &peer_id,
                my_open_mls.sign(RequestMessage::Update {
                    group_id: group_id.to_vec(),
                    epoch,
                })?,
            );
            #[cfg(feature = "bench")]
            archive_query_timing::started(_req_id);
        }
        Ok(())
    }

    // Register new identity at authority
    //This Function does not sign the Request, Since no Keys are available
    // The join goes through the relay, from a throwaway identity, see join_relay.
    fn register_identity(
        &mut self,
        peer_id: PeerId,
        identity: String,
    ) -> Result<(), AbstSwarmError> {
        join_relay::start();
        join_relay::send(
            peer_id,
            Request {
                body: RequestMessage::VerifyIdentity {
                    identity,
                    with_signature: true,
                },
                signature: None,
            },
        );
        Ok(())
    }

    /// Claims a free slot of `epoch` in the slots.json next to the member's key file.
    fn claim_slot(member_dir: &Path, epoch: u64, k: usize) -> Result<u32, AbstSwarmError> {
        let mut slots = SlotStore::load(member_dir);
        let slot = slots.claim(epoch, k).ok_or(AbstSwarmError::NoFreeSlot {
            dir: member_dir.display().to_string(),
            epoch,
        })?;
        slots
            .save(member_dir)
            .map_err(|e| AbstSwarmError::SlotStoreError { reason: e.to_string() })?;
        Ok(slot)
    }

    // Register this Peer at authority with existing identity
    fn peer_from_sig(
        &mut self,
        my_open_mls: &MyOpenMls,
        peer_id: PeerId,
        sig_file_path: String,
    ) -> Result<(), AbstSwarmError> {
        let path = PathBuf::from_str(&sig_file_path).map_err(|_| KeyMaterialFileError)?;
        let member_dir = path.parent().map(PathBuf::from).unwrap_or_default();
        let new_key_store =
            key_store::StoredKeyMaterial::from_file(path).map_err(|_| KeyMaterialFileError)?;
        my_open_mls.set_grpkey(
            new_key_store
                .get_grpkey()
                .ok_or(KeyMaterialFileError)?
                .to_bytes(),
        )?;
        my_open_mls.set_memkey(
            new_key_store
                .get_memkey()
                .ok_or(KeyMaterialFileError)?
                .to_bytes(),
        )?;
        my_open_mls.set_id(new_key_store.get_id().ok_or(KeyMaterialFileError)?)?;
        self.send_peer_registration(my_open_mls, peer_id, &member_dir)
    }

    /// Registers this node's own PeerID as a sharing peer of the member it just joined as.
    fn register_self(
        &mut self,
        my_open_mls: &MyOpenMls,
        authority: PeerId,
    ) -> Result<(), AbstSwarmError> {
        self.send_peer_registration(my_open_mls, authority, &my_open_mls.key_dir())
    }

    /// Registers this node as a sharing peer with one of the member's k slots of the current
    /// epoch, recorded in the slots.json in `member_dir`.
    fn send_peer_registration(
        &mut self,
        my_open_mls: &MyOpenMls,
        peer_id: PeerId,
        member_dir: &Path,
    ) -> Result<(), AbstSwarmError> {
        let (epoch, k) = my_open_mls.epoch_policy().ok_or(AbstSwarmError::NoEpochPolicy)?;
        let slot = Self::claim_slot(member_dir, epoch, k)?;
        let keypackage = my_open_mls.generate_key_package()?;
        let keypackage =
            bincode::serialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
        let (enc_shares, commits, tag, proof) =
            my_open_mls.prepare_peer_registration(&keypackage, epoch, slot)?;
        log::info!("Registering with slot {slot} of epoch {epoch}");
        let body = RequestMessage::AddPeerToSig {
            keypackage,
            enc_shares,
            commits,
            tag,
            proof,
        };
        self.send_request(&peer_id, my_open_mls.sign(body)?);
        Ok(())
    }

    /// Keeps this sharing peer registered in the next epoch with a slot of that epoch, recorded
    /// in the slots.json next to the member's key file.
    fn renew(
        &mut self,
        my_open_mls: &MyOpenMls,
        authority: PeerId,
        sig_file_path: String,
    ) -> Result<(), AbstSwarmError> {
        let path = PathBuf::from_str(&sig_file_path).map_err(|_| KeyMaterialFileError)?;
        let member_dir = path.parent().map(PathBuf::from).unwrap_or_default();
        let (epoch, k) = my_open_mls.epoch_policy().ok_or(AbstSwarmError::NoEpochPolicy)?;
        let slot = Self::claim_slot(&member_dir, epoch + 1, k)?;
        let (tag, proof) =
            my_open_mls.prove_renewal(&my_open_mls.get_peerid().to_bytes(), epoch + 1, slot)?;
        log::info!("Renewing with slot {slot} of epoch {}", epoch + 1);
        self.send_request(
            &authority,
            Request {
                body: RequestMessage::RenewPeer { tag, proof },
                signature: None,
            },
        );
        Ok(())
    }

    fn get_opener_keys(&mut self, peer_id: PeerId) -> Result<(), AbstSwarmError> {
        let request = Request {
            body: RequestMessage::RequestOpenerKeys,
            signature: None,
        };
        self.send_request(&peer_id, request);
        Ok(())
    }

    // Group Owner publishes commit with all
    fn publish_commit(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_id: Vec<u8>,
        peer_ids: Vec<PeerId>,
        commit: Vec<u8>,
        welcome_option: Option<Vec<u8>>,
        epoch: u64,
    ) -> Result<(), AbstSwarmError> {
        my_open_mls.store_to_file()?;
        let body = MessageBody::Commit {
            group_id: group_id.clone(),
            commit,
            welcome_option: welcome_option.clone(),
            epoch,
        };
        if let Some(welcome) = welcome_option.clone() {
            self.send_out_welcomes(
                my_open_mls,
                peer_ids,
                welcome,
                my_open_mls.get_ratchet_tree(&group_id)?,
            )?;
        }
        self.send(body, my_open_mls)
    }

    /*
    This filters and sorts the update response of an archive to make sure that commits and proposals are added in the right order
     */
    fn filter_and_group_messages(
        messages: Vec<MessageBody>,
        general_group_id: Vec<u8>,
    ) -> Result<Vec<(Vec<MessageBody>, MessageBody)>, AbstSwarmError> {
        //The following error handling is only helpful for a benign archive-response.
        //Malicious archives could manipulate the outer epochs to not match the epochs in the messages.
        //Such errors are detected by MLS itself.

        // Separate proposals and commits
        let mut proposals: Vec<MessageBody> = Vec::new();
        let mut commits: Vec<MessageBody> = Vec::new();

        for message in messages {
            match message {
                MessageBody::Proposal { .. } => proposals.push(message),
                MessageBody::Commit { .. } => commits.push(message),
                _ => {}
            }
        }

        // Group proposals and commits by epoch using a BTreeMap
        let mut proposals_by_epoch: BTreeMap<u64, Vec<MessageBody>> = BTreeMap::new();
        let mut commits_by_epoch: BTreeMap<u64, MessageBody> = BTreeMap::new();

        for proposal in proposals {
            if let MessageBody::Proposal {
                epoch, group_id, ..
            } = proposal.clone()
            {
                if general_group_id == group_id {
                    proposals_by_epoch.entry(epoch).or_default().push(proposal);
                }
            }
        }

        for commit in commits {
            if let MessageBody::Commit {
                epoch, group_id, ..
            } = commit.clone()
            {
                if group_id == general_group_id {
                    commits_by_epoch.insert(epoch, commit);
                }
            }
        }

        // Collect the proposals and commits ensuring strictly rising order of epochs
        let mut result: Vec<(Vec<MessageBody>, MessageBody)> = Vec::new();
        let mut last_epoch: Option<u64> = None;

        for (epoch, commit) in commits_by_epoch {
            if let Some(last) = last_epoch {
                if epoch != last + 1 {
                    return Err(HandleUpdateError {
                        reason: "Commit epochs not in strictly rising order".to_string(),
                    });
                }
            }

            if let Some(proposals) = proposals_by_epoch.get(&(epoch - 1)) {
                result.push((proposals.clone(), commit));
            } else {
                result.push((Vec::new(), commit));
            }
            last_epoch = Some(epoch);
        }

        Ok(result)
    }

    // handle the Group update response from an archive
    fn handle_update(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_id: Vec<u8>,
        messages: Vec<MessageBody>,
    ) -> Result<(), AbstSwarmError> {
        let filtered = Self::filter_and_group_messages(messages, group_id.clone())?;

        for (proposals, commit) in filtered {
            //Insert all missed proposals into proposal store
            for proposal in proposals {
                if let MessageBody::Proposal { data, .. } = proposal {
                    my_open_mls
                        .store_proposal(group_id.clone(), data)
                        .map_err(|e| HandleUpdateError {
                            reason: e.to_string(),
                        })?;
                }
            }
            //Commit
            if let MessageBody::Commit {
                group_id: commit_group_id,
                commit: commit_vec,
                welcome_option,
                ..
            } = commit
            {
                if self.handle_commit(my_open_mls, commit_group_id, commit_vec, welcome_option)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    //Handles Commit returns true if self has been removed in commit
    fn handle_commit(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_id: Vec<u8>,
        commit: Vec<u8>,
        welcome_option: Option<Vec<u8>>,
    ) -> Result<bool, AbstSwarmError> {
        let (peer_ids, ratchet_tree, self_removed) =
            match my_open_mls.handle_commit(group_id.clone(), commit) {
                Ok(val) => val,
                Err(GroupNotAvailable(_)) => {
                    //Ignore message for groups, that I do not have
                    return Ok(false);
                }
                Err(e) => return Err(MLSError(e)),
            };
        if !peer_ids.is_empty() {
            log::info!("Proposed Members have been added to group {group_id:?}, sending out welcome messages");
            if let Some(welcome) = welcome_option {
                self.send_out_welcomes(my_open_mls, peer_ids, welcome, ratchet_tree)?
            }
        }
        if self_removed {
            log::info!(
                "Removed from Group {:?}, via commit. \n Deleting Group",
                group_id
            );
            my_open_mls.remove_mls_group(group_id.clone());
            self.unsubscribe(group_id.clone(), my_open_mls)
                .map_err(|e| HandleUpdateError {
                    reason: e.to_string(),
                })?;
        }
        my_open_mls.store_to_file()?;
        Ok(self_removed)
    }

    fn handle_pubsub_event(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        event: gossipsub::Event,
    ) -> Result<(), AbstSwarmError> {
        match event {
            gossipsub::Event::Message {
                propagation_source: peer_id,
                message_id: id,
                message,
            } => {
                // Log raw gossipsub payload size (on-application payload pre-libp2p overhead)
                log::info!("Received gossipsub payload: {} bytes from {peer_id}", message.data.len());
                // A message that went out through the stem is now in GossipSub:
                // the embargo is no longer needed, and this peer's own message
                // coming back is forwarded but not shown again.
                let stem_id = stem::message_id(&message.data);
                let own = {
                    let mut state = stem_relay::state();
                    state.in_gossip(stem_id);
                    state.is_own(stem_id)
                };
                let arrived_on = message.topic.clone();
                let message: Message = bincode::deserialize(&message.data)
                    .map_err(|e| SerializationError(e.to_string()))?;
                // The signed body names its channel: a broadcast replayed on
                // another channel is rejected and not forwarded.
                if let MessageBody::BroadcastEnc { channel, .. } = &message.body {
                    if IdentTopic::new(channel.clone()).hash() != arrived_on {
                        log::warn!("Rejecting a broadcast for {channel} that arrived on {arrived_on} from {peer_id}");
                        self.gossipsub
                            .report_message_validation_result(&id, &peer_id, MessageAcceptance::Reject)
                            .map_err(|_| MessageValidationError)?;
                        return Ok(());
                    }
                }
                let (message_body, signature, attributed_to) = match my_open_mls.verify_message(message) {
                    Ok(message) => {
                        self.gossipsub
                            .report_message_validation_result(
                                &id,
                                &peer_id,
                                MessageAcceptance::Accept,
                            )
                            .map_err(|_| MessageValidationError)?;
                        if own {
                            return Ok(());
                        }
                        message
                    }
                    Err(e) => {
                        return match e {
                            MyOpenMLSError::SignatureInvalid { signature: mysig } => {
                                log::debug!(
                                    "Got a Message with Invalid Signature: {}",
                                    hex::encode(mysig.as_ref().to_bytes())
                                );
                                self.gossipsub
                                    .report_message_validation_result(
                                        &id,
                                        &peer_id,
                                        MessageAcceptance::Reject,
                                    )
                                    .map_err(|_| MessageValidationError)?;
                                Ok(())
                            }
                            e => Err(MLSError(e)),
                        }
                    }
                };

                self.deliver(notification_tx, my_open_mls, message_body, signature, attributed_to, peer_id)?;
            }
            _ => log::debug!("Received Pubsub event:  {event:?}"),
        }

        Ok(())
    }

    /// Hands a verified gossip message to this peer: decrypts what it can read,
    /// delivers subscribed broadcasts and group messages, applies MLS proposals
    /// and commits.
    fn deliver(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        message_body: MessageBody,
        signature: thressig::structs::Signature,
        attributed_to: Option<usize>,
        peer_id: PeerId,
    ) -> Result<(), AbstSwarmError> {
        match message_body {
            MessageBody::Broadcast { .. } => {} // Peers should only receive encrypted messages
            MessageBody::GroupEnc {
                group_id,
                enc_data: data,
                nonce,
            } => {
                //Validate Group Signature
                let decrypt_res = my_open_mls.decrypt_with_group_key(group_id, data, nonce);
                match decrypt_res {
                    Ok((group_number_option, data)) => {
                        let group_number = group_number_option.ok_or(GroupNotAvailable("".to_string()))?;
                        let message_body = MessageBody::Group {group_number, data};
                        Self::notify(notification_tx, Notification::Data{propagation_source:peer_id, message_body, signature:signature.to_bytes(), attributed_to})
                    }
                    Err(e) => match e {
                        GroupNotAvailable(_) => {}, //Do nothing, because not my group
                        MyOpenMLSError::SignatureInvalid{signature} => log::error!("Received message with invalid Signature: {}", hex::encode(signature.as_ref().to_bytes())),
                        SerializationError(e) => log::error!("Received message but could not decode: {e}"),
                        _ => log::error!("Received a gossipsub message, which caused an unhandeled error: {e}"),
                    }
                }
            }

            MessageBody::BroadcastEnc { channel, data, nonce } => {
                // Handle public message. The topic is inside the
                // ciphertext: decrypt every broadcast on a subscribed
                // channel and filter by topic here.
                let (topic, data) = match my_open_mls.open_broadcast(&channel, data, nonce) {
                    Ok(opened) => opened,
                    Err(e @ MyOpenMLSError::ChannelMismatch { .. }) => {
                        log::warn!("Dropping broadcast from {peer_id}: {e}");
                        return Ok(());
                    }
                    Err(e) => return Err(MLSError(e)),
                };
                if my_open_mls.subscibed_to_topic(topic.as_bytes().to_vec()) {
                    let message_body = MessageBody::Broadcast { topic, data };
                    Self::notify(
                        notification_tx,
                        Notification::Data {
                            propagation_source: peer_id,
                            message_body: message_body.clone(),
                            signature: signature.to_bytes(),
                            attributed_to,
                        },
                    )
                } else {
                    log::debug!("Ignoring a broadcast on {channel} from {peer_id}: not subscribed to its topic (run `sub <topic>` to receive)");
                }
            }
            MessageBody::Group { .. } => {} // Peers should only receive encrypted messages // Handle private message
            MessageBody::Proposal { group_id, data, .. } => {
                // Handle group proposal
                log::info!("Got a Proposal for {group_id:?}");
                if let Err(e) = my_open_mls.store_proposal(group_id, data) {
                    match e {
                        GroupNotAvailable(_) => { /* Ignore Groups that I do not belong */ }
                        _ => return Err(MLSError(e)),
                    }
                }
            }
            MessageBody::Commit {
                group_id,
                commit,
                welcome_option,
                ..
            } => {
                // Handle group commit
                log::info!("Got a Commit for {group_id:?}");
                self.handle_commit(my_open_mls, group_id, commit, welcome_option)?;
            }
        }
        Ok(())
    }

    // Send out welcome message to peers added to a group
    fn send_out_welcomes(
        &mut self,
        my_open_mls: &MyOpenMls,
        peer_ids: Vec<PeerId>,
        welcome: Vec<u8>,
        ratchet_tree: Vec<u8>,
    ) -> Result<(), AbstSwarmError> {
        for peer_id in peer_ids {
            self.send_request(
                &peer_id,
                my_open_mls.sign(RequestMessage::JoinGroup {
                    welcome: welcome.clone(),
                    ratchet_tree: ratchet_tree.clone(),
                })?,
            );
        }
        Ok(())
    }

    /// When we receive IdentityInfo, if the peer supports our Kademlia protocol, we add
    /// their listen addresses to the DHT, so they will be propagated to other peers.
    fn handle_identify_event(
        &mut self,
        identify_event: Box<identify::Event>,
    ) -> Result<(), AbstSwarmError> {
        log::debug!("Received identify::Event: {:?}", *identify_event);

        if let identify::Event::Received {
            peer_id,
            info:
                identify::Info {
                    listen_addrs,
                    protocols,
                    ..
                },
        } = *identify_event
        {
            if protocols.contains(&STEM_PROTOCOL) {
                stem_relay::capable().insert(peer_id);
            }
            if protocols.contains(&PROTOCOL_NAME) {
                for addr in listen_addrs {
                    // Skip addresses no remote host can dial (loopback, Docker bridges).
                    if is_unroutable_addr(&addr) {
                        log::debug!(
                            "Ignoring unroutable addr from identify for {}: {}",
                            peer_id,
                            addr
                        );
                        continue;
                    }
                    log::debug!("Adding received IdentifyInfo matching protocol to the DHT. Peer: {}, addr: {}", peer_id, addr);
                    self.kademlia.add_address(&peer_id, addr);
                }
            }
        }

        Ok(())
    }

    /// Key Package Requests, Identity Registration and Archive Updates
    /// Accepts the authority's archive keys only after every trusted archive has confirmed its
    /// own key, so a dishonest authority cannot hand out keys it holds and open signatures alone.
    fn verify_archive_keys(
        &mut self,
        keys: Vec<(PeerId, thressig::G2)>,
        threshold: usize,
        join: Option<archive_keys::PendingJoin>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        if threshold == 0 || threshold > keys.len() {
            log::error!(
                "Authority sent threshold {threshold} for {} archives; not using its keys",
                keys.len()
            );
            return Ok(());
        }
        if threshold <= keys.len() / 2 {
            log::warn!(
                "Opening threshold {threshold} of {}: a minority of archives can open signatures",
                keys.len()
            );
        }
        let trusted = archive_keys::trusted();
        if trusted.is_empty() {
            log::warn!(
                "No trusted archives configured (--archive): accepting the authority's archive keys unverified"
            );
            return self.accept_archive_keys(keys, threshold, join, my_open_mls);
        }
        let mut listed = std::collections::HashSet::new();
        for (archive, _) in &keys {
            if !trusted.contains(archive) || !listed.insert(*archive) {
                log::error!(
                    "Authority listed archive {archive}, which is untrusted or listed twice; not using its keys"
                );
                return Ok(());
            }
        }
        for (archive, _) in &keys {
            let request = Request {
                body: RequestMessage::RequestOpenerKeys,
                signature: None,
            };
            // During a join, the archives would otherwise see this node's PeerID ask just as
            // the AA sees the member join.
            if join.is_some() {
                join_relay::send(*archive, request);
            } else {
                self.send_request(archive, request);
            }
        }
        *archive_keys::pending().lock().unwrap() = Some(archive_keys::Pending {
            keys,
            threshold,
            confirmed: std::collections::HashSet::new(),
            join,
        });
        Ok(())
    }

    fn confirm_archive_key(
        &mut self,
        archive: PeerId,
        elgamal_pk: thressig::G2,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        let ready = {
            let mut guard = archive_keys::pending().lock().unwrap();
            let Some(pending) = guard.as_mut() else {
                return Ok(());
            };
            let matches = match pending.keys.iter().find(|(p, _)| *p == archive) {
                Some((_, claimed)) => *claimed == elgamal_pk,
                None => return Ok(()),
            };
            if !matches {
                log::error!(
                    "Archive {archive} reports a different key than the authority; not using the authority's keys"
                );
                *guard = None;
                return Ok(());
            }
            pending.confirmed.insert(archive);
            if pending.confirmed.len() < pending.keys.len() {
                return Ok(());
            }
            guard.take()
        };
        if let Some(pending) = ready {
            log::info!("All {} archive keys confirmed by the archives", pending.keys.len());
            self.accept_archive_keys(pending.keys, pending.threshold, pending.join, my_open_mls)?;
        }
        Ok(())
    }

    fn accept_archive_keys(
        &mut self,
        keys: Vec<(PeerId, thressig::G2)>,
        threshold: usize,
        join: Option<archive_keys::PendingJoin>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        let keys: Vec<thressig::G2> = keys.into_iter().map(|(_, key)| key).collect();
        my_open_mls.set_opener_threshold(threshold);
        my_open_mls.add_opener_pubkeys(&keys)?;
        if let Some(join) = join {
            self.send_join_key_material(join, my_open_mls)?;
        }
        Ok(())
    }

    fn send_join_key_material(
        &mut self,
        join: archive_keys::PendingJoin,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        let (g_sk, h_sk, pi1, pi2, pi3, enc_shares, commits) =
            my_open_mls.generate_join_key_material(join.id, join.n, join.group_key)?;
        my_open_mls.set_id(join.id)?;
        join_relay::send(
            join.authority,
            Request {
                body: RequestMessage::JoinKeyMaterial {
                    g_sk,
                    h_sk,
                    pi1,
                    pi2,
                    pi3,
                    enc_shares,
                    commits,
                },
                signature: None,
            },
        );
        Ok(())
    }

    fn handle_request_response_event(
        &mut self,
        request_event: request_response::Event<Request, Response>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        log::debug!("Received Request::Event: {:?}", request_event);
        if let request_response::Event::OutboundFailure { request_id, .. } = &request_event {
            outstanding::take(request_id);
        }
        if let request_response::Event::Message { peer, message } = request_event {
            match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    let (request_body, _) = match my_open_mls.verify(request) {
                        Ok(message) => message,
                        Err(e) => {
                            return match e {
                                MyOpenMLSError::SignatureInvalid { signature: mysig } => {
                                    log::debug!(
                                        "Got a Request with Invalid Signature: {}",
                                        hex::encode(mysig.as_ref().to_bytes())
                                    );
                                    Ok(())
                                }
                                e => Err(MLSError(e)),
                            }
                        }
                    };
                    match request_body {
                        RequestMessage::KeyPackage => {
                            log::info!(
                                "Received KeyPackage request from Peer {}",
                                peer.to_string()
                            );
                            let keypackage = my_open_mls.generate_key_package()?;
                            self.request_response
                                .send_response(
                                    channel,
                                    my_open_mls.sign(ResponseMessage::KeyPackage(
                                        bincode::serialize(&keypackage)
                                            .map_err(|x| SerializationError(x.to_string()))?,
                                    ))?,
                                )
                                .map_err(|_| SendKeyPackageError)?
                        }
                        RequestMessage::JoinGroup {
                            welcome,
                            ratchet_tree,
                        } => {
                            log::info!(
                                "Received a JoinGroup Request from Peer {}",
                                peer.to_string()
                            );
                            let group_id =
                                my_open_mls.add_group_from_welcome(welcome, ratchet_tree)?;
                            //If from authority it is the common group
                            if my_open_mls.get_authorityid() == peer {
                                my_open_mls.move_to_common_id(group_id.clone())?;
                            }
                            log::info!("Added Group {group_id:?}");
                            self.subscribe(group_id.clone(), my_open_mls)?;
                            self.request_response
                                .send_response(
                                    channel,
                                    my_open_mls.sign(ResponseMessage::JoinGroup)?,
                                )
                                .map_err(|_| SendJoinGroupError { group_id })?
                        }
                        _ => {}
                    }
                }
                request_response::Message::Response {
                    response,
                    request_id,
                    ..
                } => {
                    // Round trip of an archive query, measured in bench builds only.
                    #[cfg(feature = "bench")]
                    let rtt = archive_query_timing::finished(&request_id);
                    #[cfg(not(feature = "bench"))]
                    let rtt: Option<std::time::Duration> = None;
                    match outstanding::take(&request_id) {
                        Some(asked) if asked.answered_by(&response.body) => {
                            self.handle_response(peer, response, rtt, my_open_mls)?
                        }
                        asked => log::warn!(
                            "Ignoring a response from {peer} that does not answer its request ({asked:?})"
                        ),
                    }
                }
            }
        }
        Ok(())
    }

    /// Responses from the main identity, or relayed from the join's throwaway identity.
    fn handle_response(
        &mut self,
        peer: PeerId,
        response: Response,
        _rtt: Option<std::time::Duration>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        match response.body {
            ResponseMessage::KeyPackage(ref val) => {
                //Only Verify KPs
                if let Err(e) = my_open_mls.verify(response.clone()) {
                    return match e {
                        MyOpenMLSError::SignatureInvalid { signature: mysig } => {
                            log::debug!(
                                "Got a Response with Invalid Signature: {}",
                                hex::encode(mysig.as_ref().to_bytes())
                            );
                            Ok(())
                        }
                        e => Err(MLSError(e)),
                    };
                };
                log::info!("Response with KeyPackage from {}", peer.to_string());
                let keypackage: KeyPackage = bincode::deserialize(val)
                    .map_err(|x| SerializationError(x.to_string()))?;
                if let Err(e) = my_open_mls.insert_foreign_kp(peer, keypackage) {
                    log::warn!("Not using the KeyPackage from {peer}: {e}");
                }
            }
            ResponseMessage::JoinGroup => {
                //Nothing to do for now
            }
            ResponseMessage::AddPeerShare => {
                //Nothing to do for now
            }
            ResponseMessage::OpenPending | ResponseMessage::OpenPairingsReceived => {} //Archive and authority only
            ResponseMessage::EncSharePairings { .. } => {
                //Nothing to do for now
            }
            ResponseMessage::ResUpdate { group_id, messages } => {
                log::info!(
                    "Got an update for group ´{group_id:?}´ with {} messages",
                    messages.len()
                );
                #[cfg(feature = "bench")]
                if let Some(rtt) = _rtt {
                    // Round trip as the querying peer sees it: request
                    // out, archive's Postgres lookup, response back.
                    println!(
                        "ARCHIVE_QUERY_RX messages={} latency_ms={}",
                        messages.len(),
                        rtt.as_millis()
                    );
                }
                self.handle_update(my_open_mls, group_id, messages)?;
            }
            ResponseMessage::IdentityVerified {
                id,
                n,
                group_key,
                opener_pubkeys,
                threshold,
                epoch_secs,
                peers_per_member,
            } => {
                if peer != my_open_mls.get_authorityid() {
                    log::warn!("Ignoring an identity verification from {peer}, which is not the authority");
                    return Ok(());
                }
                log::info!("Identity was verified, proceeding with memkey creation.");
                my_open_mls.set_epoch_policy(epoch_secs, peers_per_member);
                let (Some(n), Some(opener_pubkeys), Some(threshold)) =
                    (n, opener_pubkeys, threshold)
                else {
                    return Ok(());
                };
                let join = archive_keys::PendingJoin {
                    authority: peer,
                    id,
                    n,
                    group_key,
                };
                self.verify_archive_keys(
                    opener_pubkeys,
                    threshold,
                    Some(join),
                    my_open_mls,
                )?;
            }
            ResponseMessage::MemKey { ref partial_memkey } => {
                // Only the AA hands out member keys, signed, and only during this node's join.
                // add_partial_memkey then checks the credential against the member's sk.
                if peer != my_open_mls.get_authorityid() || !join_relay::active() {
                    log::warn!("Ignoring a member key from {peer}: not the authority, or no join in progress");
                    return Ok(());
                }
                if let Err(e) = my_open_mls.verify(response.clone()) {
                    log::warn!("Ignoring a member key from {peer} without a valid signature: {e}");
                    return Ok(());
                }
                if let Some(memkey) = partial_memkey.clone() {
                    my_open_mls.add_partial_memkey(memkey)?;
                    join_relay::finish();
                    log::info!("Joined as a member; registering this node as a sharing peer");
                    self.register_self(my_open_mls, peer)?;
                }
            }
            ResponseMessage::PeerAdded => {}
            ResponseMessage::OpenerKeys {
                opener_pubkeys,
                threshold,
                epoch_secs,
                peers_per_member,
            } => {
                if peer != my_open_mls.get_authorityid() {
                    log::warn!("Ignoring opener keys from {peer}, which is not the authority");
                    return Ok(());
                }
                log::info!("Got Opener Keys.");
                my_open_mls.set_epoch_policy(epoch_secs, peers_per_member);
                let Some(threshold) = threshold else {
                    log::error!("Authority sent no opening threshold; end its archive init phase first");
                    return Ok(());
                };
                self.verify_archive_keys(opener_pubkeys, threshold, None, my_open_mls)?;
            }
            ResponseMessage::ArchiveOpenerKey { elgamal_pk } => {
                self.confirm_archive_key(peer, elgamal_pk, my_open_mls)?;
            }
            ResponseMessage::PeerRefused { reason } => {
                log::warn!("Authority refused this peer: {reason}");
            }
            ResponseMessage::PeerRenewed { until_epoch } => {
                log::info!("Registration renewed until epoch {until_epoch}");
            }
            ResponseMessage::RenewRefused { reason } => {
                log::warn!("Authority refused to renew this peer: {reason}");
            }
        }
        Ok(())
    }

    fn handle_non_functional_event(
        &mut self,
        event: SwarmEvent<<MyBehaviour as NetworkBehaviour>::ToSwarm>,
    ) -> Result<(), AbstSwarmError> {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                log::info!("Listening on {address:?}")
            }
            SwarmEvent::OutgoingConnectionError {
                connection_id: _connection_id,
                peer_id,
                error,
            } => {
                // Reduce noise for expected transient errors, still log with context
                match peer_id {
                    None => log::warn!("Outgoing connection error (no peer id): {}", error),
                    Some(peer_id) => {
                        log::error!("Could not connect to peer '{}': {}", peer_id, error)
                    }
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, num_established, .. } => {
                if num_established.get() == 1 {
                    if endpoint.is_dialer() {
                        stem_relay::outbound().insert(peer_id);
                    } else {
                        stem_relay::outbound().remove(&peer_id);
                    }
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, num_established, .. } => {
                if num_established == 0 {
                    stem_relay::outbound().remove(&peer_id);
                }
                if let Some(connerror) = cause {
                    log::error!(
                        "Connection to peer {} closed because of an error: {}",
                        peer_id,
                        connerror
                    );
                } else {
                    log::debug!("Connection to peer {} closed normally", peer_id);
                }
            }
            _ => log::trace!("Unhandled Swarm event: {event:?}"),
        }
        Ok(())
    }
}

#[async_trait]
impl InstructionHandler for MyBehaviour {
    async fn handle_instruction(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        instruction: Instruction,
    ) {
        match instruction {
            Instruction::Send { message } => {
                if let Err(error) = self.send(message, my_open_mls) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::SendAttributed { message } => {
                if let Err(error) = self.send_signed(message, my_open_mls, true) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::Subscribe(topic) => {
                if let Err(error) = self.subscribe(topic, my_open_mls) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::UnSubscribe(topic) => {
                if let Err(error) = self.unsubscribe(topic.as_bytes().to_vec(), my_open_mls) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::RequestKPs(peer_ids) => {
                if let Err(error) = self.get_keypackages(my_open_mls, peer_ids) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::CreateGroup(peer_ids) => {
                if let Err(error) = self.create_group(peer_ids, my_open_mls) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::ListGroups => {
                if let Err(error) = self.list_groups(my_open_mls) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::AddGroupMember {
                group_number,
                peer_id,
            } => {
                if let Err(error) = self.add_group_member(my_open_mls, group_number, peer_id) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::RemoveGroupMember {
                group_number,
                peer_id,
            } => {
                if let Err(error) = self.remove_group_member(my_open_mls, group_number, peer_id) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::DeleteGroupForAll { group_number } => {
                if let Err(error) = self.delete_group_for_all(my_open_mls, group_number) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::UpdateKeyMaterial { group_number } => {
                if let Err(error) = self.update_key_material(my_open_mls, group_number) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::Commit {
                group_id,
                peer_ids,
                commit,
                welcome,
                epoch,
            } => {
                if let Err(error) = self.publish_commit(
                    my_open_mls,
                    group_id.to_vec(),
                    peer_ids,
                    commit,
                    welcome,
                    epoch,
                ) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::UpdateGroups { peer_id } => {
                if let Err(error) = self.update_groups_req(my_open_mls, peer_id) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::RegisterIdentity { peer_id, identity } => {
                if let Err(error) = self.register_identity(peer_id, identity) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::PeerFromSig {
                peer_id,
                sig_file_path,
            } => {
                if let Err(error) = self.peer_from_sig(my_open_mls, peer_id, sig_file_path) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::GetOpenerKeys { peer_id } => {
                if let Err(error) = self.get_opener_keys(peer_id) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::RenewPeer {
                peer_id,
                sig_file_path,
            } => {
                if let Err(error) = self.renew(my_open_mls, peer_id, sig_file_path) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::ReqOpen { .. } => {} //Ignore
            Instruction::Open { .. } => {}    //Ignore
            Instruction::EndInitArchive => {} //Ignore
            Instruction::ListPendingOpens
            | Instruction::ApproveOpen { .. }
            | Instruction::DenyOpen { .. } => {} //Archive-only
        }
    }
}
#[async_trait]
impl EventHandler for MyBehaviour {
    async fn handle_event(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        event: SwarmEvent<Self::ToSwarm>,
    ) {
        let connected = match &event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => Some(*peer_id),
            _ => None,
        };
        if let Err(e) = match event {
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(event)) => {
                self.handle_pubsub_event(notification_tx, my_open_mls, *event)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::IdentifyEvent(e)) => {
                self.handle_identify_event(e)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::RequestResponseEvent(e)) => {
                self.handle_request_response_event(*e, my_open_mls)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Stem(e)) => {
                self.handle_stem_event(notification_tx, *e, my_open_mls)
            }
            non_functional_event => self.handle_non_functional_event(non_functional_event),
        } {
            Self::notify_error(notification_tx, e);
        }
        admission::update(&mut self.gossipsub, my_open_mls, connected.as_ref());
    }

    async fn handle_relayed_response(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        peer: PeerId,
        response: Response,
    ) {
        if let Err(e) = self.handle_response(peer, response, None, my_open_mls) {
            Self::notify_error(notification_tx, e);
        }
    }
}

#[derive(Debug)]
pub(crate) enum MyBehaviourEvent {
    Gossipsub(Box<gossipsub::Event>),
    KademliaEvent(()),
    IdentifyEvent(Box<identify::Event>),
    RequestResponseEvent(Box<request_response::Event<Request, Response>>),
    Stem(Box<request_response::Event<StemMessage, StemAck>>),
}

impl From<request_response::Event<StemMessage, StemAck>> for MyBehaviourEvent {
    fn from(event: request_response::Event<StemMessage, StemAck>) -> Self {
        MyBehaviourEvent::Stem(Box::new(event))
    }
}

impl From<std::convert::Infallible> for MyBehaviourEvent {
    fn from(event: std::convert::Infallible) -> Self {
        match event {}
    }
}

impl From<gossipsub::Event> for MyBehaviourEvent {
    fn from(event: gossipsub::Event) -> Self {
        MyBehaviourEvent::Gossipsub(Box::new(event))
    }
}

impl From<kad::Event> for MyBehaviourEvent {
    fn from(_event: kad::Event) -> Self {
        MyBehaviourEvent::KademliaEvent(())
    }
}

impl From<identify::Event> for MyBehaviourEvent {
    fn from(event: identify::Event) -> Self {
        MyBehaviourEvent::IdentifyEvent(Box::new(event))
    }
}

impl From<request_response::Event<Request, Response>> for MyBehaviourEvent {
    fn from(event: request_response::Event<Request, Response>) -> Self {
        MyBehaviourEvent::RequestResponseEvent(Box::new(event))
    }
}

#[derive(Debug, Error)]
pub(crate) enum MyBehaviourError {
    // This error is deliberately generic, because we don't want to break the error API
    // when we change the pubsub behaviour.
    #[error("Could not construct composed pubsub behaviour: {0}")]
    PubSubBehaviourError(String),
    #[error("The address for the bootstrap peer is invalid: {0}")]
    InvalidBootstrapPeerAddress(#[from] libp2p::multiaddr::Error),
    #[error("The address for the bootstrap peer is invalid: {0}")]
    InvalidPeerId(#[from] ParseError),
}
