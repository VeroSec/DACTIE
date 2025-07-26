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
use crate::abstswarm::AbstSwarmError::{MLSError, SendError};
use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::instruction_handler::InstructionHandler;
use dactie_utils::mls_wrapper::{MyOpenMLSError, MyOpenMls};
use dactie_utils::mls_wrapper::MyOpenMLSError::{SerializationError};
use dactie_utils::shared_structs::*;
use dactie_utils::shared_structs::RequestMessage::RequestOpen;
use thressig::structs::{EncShare, EncSharePairing, Signature};
use crate::storage::Storage;

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

    fn send_out_id_enc_shares(&mut self, my_open_mls: &MyOpenMls, archvive_peer_ids: &[PeerId], id:usize, enc_shares: &[EncShare], commits: &Vec<thressig::G1>, sender_peer_id: PeerId) -> Result<(), AbstSwarmError>{
        for (i, peer_id) in archvive_peer_ids.into_iter().enumerate(){
            self.request_response.send_request(&peer_id,my_open_mls.sign(RequestMessage::AddIDShare {id,peer_id: sender_peer_id,enc_share: enc_shares[i].clone(),commits: commits.clone()})?);
        }
        Ok(())
    }

    fn send_out_peer_enc_shares(&mut self, my_open_mls: &MyOpenMls, archvive_peer_ids: &[PeerId], requester_peer_id: PeerId, enc_shares: &[EncShare], commits: &Vec<thressig::G1>) -> Result<(), AbstSwarmError>{
        for (i, peer_id) in archvive_peer_ids.into_iter().enumerate(){
            self.request_response.send_request(&peer_id,my_open_mls.sign(RequestMessage::AddPeerShare {peer_id:requester_peer_id,enc_share: enc_shares[i].clone(),commits: commits.clone()})?);
        }
        Ok(())
    }

    fn req_open(&mut self,storage: &Storage, my_open_mls: &MyOpenMls, signature: Vec<u8>) -> Result<(),AbstSwarmError>{
        let archive_peer_ids = storage.get_archive_peer_ids();
        for peer_id in archive_peer_ids {
            self.request_response.send_request(&peer_id,my_open_mls.sign(RequestOpen {signature:signature.clone()})?);
        }
        Ok(())
    }

    fn open(storage: &Storage, my_open_mls: &MyOpenMls, signature: Vec<u8>) -> Result<(),AbstSwarmError> {
        let signature: Signature = bincode::deserialize(&signature).map_err(|x| SerializationError(x.to_string()))?;
        let (ids, peers) = storage.open_signature(signature, my_open_mls)?;
        for (id,name) in ids {
            println!("Signature belongs to ID:{:?}, Name:{}", id,name);
        }

        for peerid in peers {
            println!("Signature belongs to PeerID:{:?}", peerid);
        }
        Ok(())
    }

    fn end_init_archive(&mut self, storage: &Storage, my_open_mls: &MyOpenMls) -> Result<(),AbstSwarmError> {
        let pubkeys = &storage.get_opener_pubkeys();
        if pubkeys.len() > 1{
            storage.end_init_archive();
            let peer = my_open_mls.get_peerid();
            let id = 0usize;
            log::info!("Enough Archives registered, ending init phase");
            let (n, group_key) = storage.return_new_n(peer,id,"Authority".to_string());
            my_open_mls.add_opener_pubkeys(pubkeys)?;
            let (g_sk, h_sk, pi1,pi2,enc_shares,commits) = my_open_mls.generate_join_key_material(id, n.unwrap(),group_key)?; // n.unwrap should be safe
            let (_,partial_memkey) = storage.return_memkey(&my_open_mls.get_peerid(),&g_sk,&h_sk,&commits,&enc_shares,pi1,pi2)?;
            my_open_mls.add_partial_memkey(partial_memkey)?;

            log::info!("Creating common group with archives...");
            let archive_ids = storage.get_archive_peer_ids();
            let group_numb = my_open_mls.create_mls_group(&archive_ids)?;
            my_open_mls.move_to_common(group_numb);
            self.send_out_id_enc_shares(my_open_mls, &archive_ids, id, &enc_shares, &commits,peer)?;
            storage.to_file()?;
        } else {
            log::error!("Not enough archives registered, please add at least two archives!");
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
                            // !!!!For testing purpose this expects a unique identity!!!!
                            RequestMessage::VerifyIdentity {identity, with_signature} => {
                                //Do important identity checks here
                                let mut hasher = DefaultHasher::new();
                                identity.hash(&mut hasher);
                                let id = hasher.finish() as usize;
                                if !identity.is_empty() && !storage.id_exists(&id){
                                    let (n, group_key) = storage.return_new_n(peer,id,identity);
                                    //Archives can only be created in the init phase
                                    let res = if with_signature && !storage.get_archive_init_mode(){
                                        let opener_pubkeys = storage.get_opener_pubkeys();
                                        ResponseMessage::IdentityVerified {id, n, group_key,opener_pubkeys:Some(opener_pubkeys)}
                                    } else {
                                        ResponseMessage::IdentityVerified {id, n: None, group_key, opener_pubkeys:None}
                                    };
                                    log::info!("Verified Identity sending out respones...");
                                    self.request_response.send_response(channel, Response{body:res,signature:None}).unwrap_or(());

                                }
                            }

                            /*
                            Manages Join Requests of Sharing Peers
                            Create Memberkey,
                            Add Peer to Commongroup
                             */
                            RequestMessage::JoinKeyMaterial {g_sk,h_sk, pi1,pi2, enc_shares,commits,keypackage} => {
                                let (id,partial_memkey) = storage.return_memkey(&peer, &g_sk,&h_sk,&commits,&enc_shares,pi1,pi2)?;
                                self.send_out_id_enc_shares(my_open_mls, &storage.get_archive_peer_ids(), id, &enc_shares, &commits,peer)?;
                                let keypackage: KeyPackage = bincode::deserialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
                                log::info!("Created Key Material adding peer to common group...");
                                storage.allow_peer(&keypackage, peer.clone())?;
                                let group_id = my_open_mls.get_common_group_id()?;
                                let (_, commit, welcome_option, epoch)
                                    = my_open_mls.add_group_members(group_id.clone(), &[(peer, keypackage.clone())])?;
                                self.publish_commit(my_open_mls,group_id.to_vec(),vec![peer],commit,welcome_option,epoch)?;
                                let res = ResponseMessage::MemKey {partial_memkey: Some(partial_memkey),/* welcome: welcome_option.unwrap(), ratchet_tree: my_open_mls.get_ratchet_tree(&group_id.to_vec())?*/};
                                self.request_response.send_response(channel, my_open_mls.sign(res)?).unwrap_or(());
                                
                            },

                            /*
                            Manages Join Requests of Archive Peers
                            Add Peer to Commongroup
                             */
                            RequestMessage::JoinKeyPackage {keypackage, elgamal_pk} => {
                                if let Some(_) = storage.remove_n(&peer){
                                    let keypackage: KeyPackage = bincode::deserialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
                                    let res = ResponseMessage::MemKey {partial_memkey: None};
                                    if let Ok(_) = self.request_response.send_response(channel, Response{body:res, signature:None}) {
                                        storage.allow_peer(&keypackage,peer)?;
                                        my_open_mls.insert_foreign_kp(peer, keypackage);
                                        storage.add_archive(peer);
                                        storage.add_opener_pubkey(elgamal_pk);
                                        storage.to_file()?;
                                    }
                                }
                            }





                            //Register Peer with existing Group Signature.
                            RequestMessage::AddPeerToSig {keypackage, enc_shares, commits} => {
                                log::info!("Got Request to add new peer from existing member");
                                match my_open_mls.verify(request) {
                                    Ok(_) => {}
                                    Err(e) => return match e {
                                        MyOpenMLSError::SignatureInvalid {signature: mysig} => {
                                            log::debug!("Got a Request with Invalid Signature: {}", hex::encode(&mysig.to_bytes()));
                                            Ok(())
                                        }
                                        e => Err(MLSError(e))

                                    },
                                };

                                let mut verified= true;
                                //Verify Keypackage belongs to Peer
                                let keypackage: KeyPackage = bincode::deserialize(&keypackage).map_err(|x| SerializationError(x.to_string()))?;
                                if !Storage::verify_key_package(&keypackage, peer.clone())?{
                                    verified = false;
                                    log::info!("Verification of new Peer from existing Sig failed, KP does not belong to Peer");
                                }

                                if verified {
                                    //Verify EncShare
                                    let opener_pubkeys= storage.get_opener_pubkeys();
                                    for (enc_share,key) in enc_shares.iter().zip(opener_pubkeys.iter()) {
                                        if my_open_mls.verify_enc_share(enc_share,&commits,Some(key.clone())) == false {
                                            verified = false;
                                            log::info!("Verification of new Peer from existing Sig failed, EncShares could not be verified");
                                            break;
                                        }
                                    };
                                }


                                if verified {
                                    log::info!("Adding new peer from existing member");
                                    self.send_out_peer_enc_shares(my_open_mls, &storage.get_archive_peer_ids(), peer, &enc_shares, &commits)?;


                                    //Add Peer to common group
                                    storage.allow_peer(&keypackage,peer)?;
                                    let group_id = my_open_mls.get_common_group_id()?;
                                    let (_, commit, welcome_option, epoch)
                                        = my_open_mls.add_group_members(group_id.clone(), &[(peer, keypackage.clone())])?;
                                    self.publish_commit(my_open_mls,group_id.to_vec(),vec![peer],commit,welcome_option,epoch)?;
                                    let res = ResponseMessage::PeerAdded;
                                    self.request_response.send_response(channel, my_open_mls.sign(res)?).unwrap_or(());

                                }

                            }
                            
                            RequestMessage::RequestOpenerKeys => {
                                let res = ResponseMessage::OpenerKeys {opener_pubkeys: storage.get_opener_pubkeys()};
                                self.request_response.send_response(channel, my_open_mls.sign(res)?).unwrap();
                            }

                            _ => {}
                        }
                    }
                    request_response::Message::Response {
                        response,..
                    } => {
                        match response.body {
                            //Ignore all other Responses for now
                            ResponseMessage::EncSharePairings {enc_share_id_pairings,enc_share_peer_pairings,signature} => {

                                let id_pairings: Vec<(usize,EncSharePairing)> = bincode::deserialize(&enc_share_id_pairings).map_err(|x| SerializationError(x.to_string()))?;


                                let peer_pairings_bytes: Vec<(Vec<u8>,EncSharePairing)> = bincode::deserialize(&enc_share_peer_pairings).map_err(|x| SerializationError(x.to_string()))?;
                                let mut peer_pairings = Vec::new();
                                for (peer_id_bytes, enc_share_pairing) in peer_pairings_bytes {
                                    let peer_id = PeerId::from_bytes(&peer_id_bytes).map_err(|e| SerializationError(e.to_string()))?;
                                    peer_pairings.push((peer_id, enc_share_pairing));

                                }


                                let signature: Signature = bincode::deserialize(&signature).map_err(|x| SerializationError(x.to_string()))?;
                                log::info!("Got {} EncShare Pairing from {}", id_pairings.len()+ peer_pairings.len(),peer);
                                storage.insert_enc_id_share_pairing(signature.clone(), &id_pairings);
                                storage.insert_enc_peer_share_pairing(signature, &peer_pairings);
                                storage.to_file()?;

                            }

                            _ => {}
                        }
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
        storage: & Storage
    ) {
        match instruction {
            Instruction::Commit {group_id,peer_ids, commit, welcome, epoch} => {
                if let Err(error) = self.publish_commit(my_open_mls, group_id.to_vec(),peer_ids, commit, welcome, epoch){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::ReqOpen {signature} => {
                if let Err(error) = self.req_open(storage,my_open_mls,signature){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::Open {signature} => {
                if let Err(error) = Self::open(storage,my_open_mls,signature){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            
            Instruction::EndInitArchive => {
                if let Err(error) = self.end_init_archive(storage,my_open_mls){
                    Self::notify(notification_tx, Notification::Err(error));
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

