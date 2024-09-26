use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::time::Duration;
use async_trait::async_trait;
use openmls::key_packages::KeyPackage;
use thiserror::Error;
use tokio::io;
use tokio::sync::mpsc;
use libp2p::{gossipsub, identify, kad, PeerId, request_response, StreamProtocol};
use libp2p::gossipsub::IdentTopic;
use libp2p::identity::{Keypair, ParseError};
use libp2p::kad::{Mode, PROTOCOL_NAME};
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::{ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use crate::mybehaviour::MyBehaviourError::PubSubBehaviourError;

use crate::abstswarm::{AbstSwarmError, Notification};
use crate::abstswarm::AbstSwarmError::{ SendError};
use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::instruction_handler::InstructionHandler;
use dactie_utils::mls_wrapper::{MyOpenMls};
use dactie_utils::mls_wrapper::MyOpenMLSError::{SerializationError};
use dactie_utils::shared_structs::*;
use crate::storage::Storage;
use crate::storage::MyStorageError::{SignatureOpenError};

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
    pub(crate) request_response: request_response::json::Behaviour<Request,Response>,
    }

impl MyBehaviour {
    pub(crate) fn new(
        keypair: &Keypair,
    ) -> Result<MyBehaviour, MyBehaviourError> {
        // To content-address message, we can take the hash of message and use it as an ID.
        let message_id_fn = |message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        };

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10)) // This is set to aid debugging by not cluttering the log space
            .validation_mode(gossipsub::ValidationMode::Permissive) // This sets the kind of message validation. Permissive allows anonymous messages
            .validate_messages()                                    // Allows validation of group signatures
            .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
            .build()
            .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg)).unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.



        let mut gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
            .map_err(|err| PubSubBehaviourError(err.to_string()))?;

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

        let request_response = request_response::json::Behaviour::new(
            iter::once((StreamProtocol::new("/key_package/1"), ProtocolSupport::Full)),
                                                                request_response::Config::default());

        Ok(Self {
            gossipsub,
            identify,
            kademlia,
            request_response,
        })
    }

    fn send(&mut self, body: MessageBody, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        let (body, channel) = match body.clone() {
            MessageBody::Commit {group_id,epoch,..} => {
                //Return early if no other group members available
                if epoch==0 {
                    return Ok(());
                }
                (body, my_open_mls.get_channel(group_id))
            },
            _ => return Err(SendError {reason: "Wrong Message Body for sending".to_string()})
        };


        let message = my_open_mls.sign(body)?;

        let to_send = bincode::serialize(&message).map_err(|e|SerializationError(e.to_string()))?;
        if let Err(error) = self
            .gossipsub
            .publish(IdentTopic::new(channel.clone()), to_send)
        {
            log::error!(
                "Failed to publish message to the pubsub topic '{}': {}",
                channel,
                error
            );
            return Err(SendError {
                reason: format!("{error}"),
            });
        }
        Ok(())
    }

    fn notify(notification_tx: &mpsc::UnboundedSender<Notification>, notification: Notification) {
        if let Err(e) = notification_tx.send(notification) {
            log::error!("Failed to send notification back to router through mpsc channel: {e}");
        }
    }

    pub fn notify_error(notification_tx: &mpsc::UnboundedSender<Notification>, error: AbstSwarmError) {
        if let Err(e) = notification_tx.send(Notification::Err(error)) {
            log::error!("Failed to send notification back to router through mpsc channel: {e}");
        }
    }


    /**
    Functions that handle the management of the Common MLS Group
    **/
    fn publish_commit(&mut self, my_open_mls: &MyOpenMls, group_id: Vec<u8>, peer_ids: Vec<PeerId>,commit: Vec<u8>, welcome_option: Option<Vec<u8>>, epoch: u64) -> Result<(), AbstSwarmError> {
        my_open_mls.store_to_file()?;
        let message = MessageBody::Commit {group_id: group_id.clone(), commit, welcome_option:welcome_option.clone(), epoch};
        if let Some(welcome) = welcome_option.clone(){
            self.send_out_welcomes(my_open_mls,peer_ids, welcome,my_open_mls.get_ratchet_tree(&group_id)?)?;
        }
        self.send(message,my_open_mls)
    }

    fn send_out_welcomes(&mut self, my_open_mls: &MyOpenMls,peer_ids: Vec<PeerId>, welcome: Vec<u8>,ratchet_tree: Vec<u8>) -> Result<(), AbstSwarmError>{
        for peer_id in peer_ids{
            self.request_response.send_request(&peer_id,my_open_mls.sign(RequestMessage::JoinGroup{welcome: welcome.clone(),ratchet_tree: ratchet_tree.clone()})?);
        }
        Ok(())
    }

    /// When we receive IdentityInfo, if the peer supports our Kademlia protocol, we add
    /// their listen addresses to the DHT, so they will be propagated to other peers.
    fn handle_identify_event(&mut self, identify_event: Box<identify::Event>) -> Result<(),AbstSwarmError> {
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
            if protocols
                .iter()
                .any(|p| *p == PROTOCOL_NAME)
            {
                for addr in listen_addrs {
                    log::debug!("Adding received IdentifyInfo matching protocol  to the DHT. Peer: {}, addr: {}", peer_id, addr);
                    self.kademlia.add_address(&peer_id, addr);
                }
            }
        }

        Ok(())
    }

    fn handle_request_response_event(&mut self, request_event: request_response::Event<Request,Response>, my_open_mls: &MyOpenMls, storage: &Storage) -> Result<(),AbstSwarmError>{
        log::debug!("Received Request::Event: {:?}", request_event);
        match request_event {
            request_response::Event::Message {
                peer,
                message
            } => {
                match message {
                    request_response::Message::Request {
                        request, channel, ..
                    } => {
                        //No signature validation, peers do not have key material
                        match request.body.clone() {

                            // Verify Identity and Return public group key if valid
                            RequestMessage::VerifyIdentity {identity, with_signature} => {
                                //Do important identity checks here
                                if !identity.is_empty() {
                                    let (n, group_key) = storage.return_new_n(peer);
                                    // Archives do not get a memberkey, therefore they do not need an n
                                    let res = if with_signature{
                                        ResponseMessage::IdentityVerified {n, group_key}
                                    } else {
                                        ResponseMessage::IdentityVerified {n: None, group_key}
                                    };
                                    log::info!("Verified Identity sending out respones...");
                                    self.request_response.send_response(channel, my_open_mls.sign(res)?).unwrap_or(());

                                }
                            }

                            /*
                            Manages Join Requests of Sharing Peers
                            Create Memberkey,
                            Add Peer to Commongroup
                            Store Peer information for opening
                             */
                            RequestMessage::JoinKeyMaterial {tau, ttau, pi: (pi1, pi2), keypackage} => {
                                let (id,partial_memkey) = storage.return_memkey(&peer, tau, ttau, pi1, pi2)?;
                                let keypackage: KeyPackage = bincode::deserialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
                                log::info!("Created Key Material adding peer to common group...");
                                storage.allow_peer(&keypackage, peer.clone())?;
                                let group_id = my_open_mls.get_common_group_id()?;
                                let (_, commit, welcome_option, epoch)
                                    = my_open_mls.add_group_members(group_id.clone(), &[(peer, keypackage.clone())])?;
                                self.publish_commit(my_open_mls,group_id.to_vec(),Vec::new(),commit,None,epoch)?;
                                let res = ResponseMessage::MemKey {partial_memkey: Some(partial_memkey), welcome: welcome_option.unwrap(), ratchet_tree: my_open_mls.get_ratchet_tree(&group_id.to_vec())?};
                                if let Ok(_) = self.request_response.send_response(channel, my_open_mls.sign(res)?){
                                    storage.add_peer_id(&id, peer, false)?;
                                }



                            },

                            /*
                            Manages Join Requests of Archive Peers
                            Add Peer to Commongroup
                             */
                            RequestMessage::JoinKeyPackage {keypackage} => {
                                if let Some(_) = storage.remove_n(&peer){
                                    let keypackage: KeyPackage = bincode::deserialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
                                    log::info!("Adding Archive to common group...");
                                    let group_id = my_open_mls.get_common_group_id()?;
                                    let ( _, commit, welcome_option, epoch)
                                        = my_open_mls.add_group_members(group_id.clone(), &[(peer, keypackage.clone())])?;
                                    self.publish_commit(my_open_mls,group_id.to_vec(),Vec::new(),commit,None,epoch)?;
                                    let res = ResponseMessage::MemKey {partial_memkey: None, welcome: welcome_option.unwrap(), ratchet_tree: my_open_mls.get_ratchet_tree(&group_id.to_vec())?};
                                    if let Ok(_) = self.request_response.send_response(channel, my_open_mls.sign(res)?) {
                                        storage.allow_peer(&keypackage,peer)?;
                                    }

                                }
                            }

                            /*
                            Register Peer with existing Group Signature.
                            */
                            RequestMessage::AddPeerToSig {keypackage} => {
                                let keypackage: KeyPackage = bincode::deserialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
                                let id = storage.open(request.signature.ok_or(SignatureOpenError)?.to_bytes())?;
                                storage.allow_peer(&keypackage,peer)?;
                                let group_id = my_open_mls.get_common_group_id()?;
                                let (_, commit, welcome_option, epoch)
                                    = my_open_mls.add_group_members(group_id.clone(), &[(peer, keypackage.clone())])?;
                                self.publish_commit(my_open_mls,group_id.to_vec(),Vec::new(),commit,None,epoch)?;
                                let res = ResponseMessage::PeerAdded{welcome: welcome_option.unwrap(), ratchet_tree: my_open_mls.get_ratchet_tree(&group_id.to_vec())?};
                                if let Ok(_) = self.request_response.send_response(channel, my_open_mls.sign(res)?) {
                                    storage.add_peer_id(&id, peer,true)?;
                                }

                            }
                            _ => {}
                        }
                    }
                    //Ignore all Responses
                    _  => {
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_non_functional_event(&mut self, event: SwarmEvent<<MyBehaviour as NetworkBehaviour>::ToSwarm>,
    ) -> Result<(),AbstSwarmError>{
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                log::info!("Listening on {address:?}")
            }
            SwarmEvent::OutgoingConnectionError { connection_id: _connection_id, peer_id, error } => match peer_id {
                None => log::error!("Could not connect: {}", error),
                Some(peer_id) => log::error!("Could not connect to peer '{}': {}", peer_id, error),
            },
            SwarmEvent::ConnectionClosed {
                peer_id,
                cause: Some(connerror)  ,
                ..
            } => {
                log::error!(
                    "Connection to peer {} closed because of an error: {}",
                    peer_id,
                    connerror
                );
            }
            _ => log::trace!("Unhandled Swarm event: {event:?}"),
        }
        Ok(())
    }

}

#[async_trait]
impl EventHandler for MyBehaviour {
    async fn handle_event(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: & MyOpenMls,
        storage: &Storage,
        event: SwarmEvent<Self::ToSwarm>,
    ) {
        if let Err(e) = match event {
            SwarmEvent::Behaviour(MyBehaviourEvent::IdentifyEvent(e)) => {
                self.handle_identify_event(e)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::RequestResponseEvent(e)) => {
                self.handle_request_response_event(e,my_open_mls, storage)
            }
            non_functional_event => self.handle_non_functional_event(non_functional_event),

        } {
            Self::notify_error(notification_tx, e);
        }
    }
}

#[async_trait]
impl InstructionHandler for MyBehaviour {
    async fn handle_instruction(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: & MyOpenMls,
        instruction: Instruction,
        storage: &Storage
    ) {
        match instruction {
            Instruction::Commit {group_id,peer_ids, commit, welcome, epoch} => {
                if let Err(error) = self.publish_commit(my_open_mls, group_id.to_vec(),peer_ids, commit, welcome, epoch){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::Open {signature} => {
                match storage.open(signature){
                    Ok(val ) => {
                        if let Some(peer_ids) = storage.get_peer_ids(&val) {
                            log::info!("Signature belongs to the following ids: {peer_ids:?}");
                        } else {
                            log::info!("Opened Signature and got {val}, but was not able to map to a PeerID");
                        }
                    }
                    Err(_) => log::info!("Was not able to open signature")
                }
            }
            //Ignore other instructions
            _ => {}

        }
    }
}


#[derive(Debug)]
pub(crate) enum MyBehaviourEvent {
    Gossipsub(()),
    KademliaEvent(()),
    IdentifyEvent(Box<identify::Event>),
    RequestResponseEvent(request_response::Event<Request,Response>),
}

impl From<gossipsub::Event> for MyBehaviourEvent {
    fn from(_event: gossipsub::Event) -> Self {
        MyBehaviourEvent::Gossipsub(())
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

impl From<request_response::Event<Request,Response>> for MyBehaviourEvent {
    fn from(event: request_response::Event<Request,Response>) -> Self {
        MyBehaviourEvent::RequestResponseEvent(event)
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

