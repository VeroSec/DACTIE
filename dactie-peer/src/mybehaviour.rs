use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use async_trait::async_trait;
use openmls::key_packages::KeyPackage;
use thiserror::Error;
use tokio::io;
use tokio::sync::mpsc;
use libp2p::{gossipsub, identify, kad, Multiaddr, PeerId, request_response, StreamProtocol};
use libp2p::gossipsub::{IdentTopic, MessageAcceptance};
use libp2p::identity::{Keypair, ParseError};
use libp2p::kad::{Mode, PROTOCOL_NAME};
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::{ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use dactie_utils::key_store;
use crate::mybehaviour::MyBehaviourError::PubSubBehaviourError;

use crate::abstswarm::{AbstSwarmError, Notification};
use crate::abstswarm::AbstSwarmError::{HandleUpdateError, MLSError, MessageValidationError, SendError, SendJoinGroupError, SendKeyPackageError};
use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::instruction_handler::InstructionHandler;
use dactie_utils::mls_wrapper::{MyOpenMls, MyOpenMLSError};
use dactie_utils::mls_wrapper::MyOpenMLSError::{GroupNotAvailable, KeyMaterialFileError, SerializationError};
use dactie_utils::shared_structs::*;

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
        boot_peer_id_option: Option<PeerId>,
        boot_peer_addr: Multiaddr,
    ) -> Result<MyBehaviour, MyBehaviourError> {
        // To content-address message, we can take the hash of message and use it as an ID.
        let message_id_fn = |message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        };

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10)) // This is set to aid debugging by not cluttering the log space
            .validation_mode(gossipsub::ValidationMode::Permissive) // This sets the kind of message validation. The default is Strict (enforce message signing)
            .validate_messages()                        // Allows validation of group signatures
            .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
            .flood_publish(false)          //Flood Pub Deactivate to provide Source Anonymity
            .build()
            .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg)).unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.


        

        let mut gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Anonymous,
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

        if let Some(boot_peer_id) = boot_peer_id_option {
            // Add the bootnodes to the local routing table.
            kademlia.add_address(&boot_peer_id, boot_peer_addr);

        }

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

    //Subscribe to Gossipsub topic
    fn subscribe(&mut self, topic: Vec<u8>, my_open_mls: &MyOpenMls) -> Result<(),AbstSwarmError>{
        // Use partitioned Channels to hide Subscriptions
        match my_open_mls.subscribe(topic.clone()){
            Ok(opt_channel) => {
                if let Some(channel) = opt_channel{
                    log::debug!("Subscribing to channel {channel:?}");
                    if let Err(error) = self.gossipsub.subscribe(&IdentTopic::new(channel.clone())){
                        log::error!(
                        "Failed to subscribe to channel '{}': {}",
                        channel,
                        error,
                    );
                        return Err(AbstSwarmError::SubscriptionError {
                            reason: format!("{error}"),
                        })
                    }
                }
            },
            Err(err) => return Err(MLSError(err))

        }
        my_open_mls.store_to_file()?;
        Ok(())
    }


    //Unsubscribe from Gossipsub topic
    fn unsubscribe(&mut self, topic: Vec<u8>, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        match my_open_mls.unsubscribe(topic) {
            Ok(opt_channel) =>  {
                if let Some(channel) = opt_channel{
                    log::debug!("Unsubscribing from channel {channel:?}");
                    if let Err(error) = self.gossipsub.unsubscribe(&IdentTopic::new(channel.clone())){
                        log::error!(
                        "Failed to unsubscribe from channel '{}': {}",
                        channel,
                        error,
                    );
                        return Err(AbstSwarmError::SubscriptionError {
                            reason: format!("{error}"),
                        })
                    }
                }
            }
            Err(err) => return Err(MLSError(err))
        }
        my_open_mls.store_to_file()?;

        Ok(())
    }


    // Send a message over gossipsub
    fn send(&mut self, body: MessageBody, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        let (body, channel) = match body.clone() {
            MessageBody::Broadcast {ref topic, data } => {
                (self.send_to_topic(topic, data, my_open_mls)?,
                my_open_mls.get_channel(topic.as_bytes().to_vec()))
            }
            MessageBody::Group {group_number, data} => {
                let (body,group_id)=self.send_to_group(my_open_mls,group_number,data)?;
                (body, my_open_mls.get_channel(group_id))
            },
            MessageBody::Commit {group_id,..} => {
                (body, my_open_mls.get_channel(group_id))
            },
            MessageBody::Proposal {group_id,..} => {
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
            match error {
                gossipsub::PublishError::InsufficientPeers => log::debug!("Could not publish, no peers subscribed to channel"),
                e => return Err(SendError {reason: format!("{e}")})
            }
        }
        Ok(())
    }

    fn send_to_topic(&mut self, topic:&String, message: Vec<u8>, my_open_mls: &MyOpenMls) -> Result<MessageBody, AbstSwarmError> {
        log::debug!("Broadcasting: {:?}",message.clone());

        let (_, ciphertext, nonce) = my_open_mls.encrypt_with_group_key(my_open_mls.get_common_group_id()?, message)?;
        Ok(MessageBody::BroadcastEnc {topic: topic.clone(), data:ciphertext, nonce})
    }

    fn send_to_group(&mut self, my_open_mls: &MyOpenMls, group_number: usize, data: Vec<u8>) -> Result<(MessageBody,Vec<u8>), AbstSwarmError> {
        let group_id = my_open_mls.get_group_id(group_number)?;
        let (group_id,ciphertext,nonce) = my_open_mls.encrypt_with_group_key(group_id,data.into())?;
        Ok((MessageBody::GroupEnc {group_id: group_id.clone(), enc_data: ciphertext, nonce}, group_id))
    }

    // Get key Pakcage from other Peers
    fn get_keypackages(&mut self, my_open_mls: &MyOpenMls,peer_ids: Vec<PeerId>) -> Result<(), AbstSwarmError> {
        for peer_id in peer_ids {
            let request: Request = my_open_mls.sign(RequestMessage::KeyPackage)?;
            self.request_response.send_request(&peer_id,request);
        }
        Ok(())
    }

    // Create new MLS Group
    fn create_group(&mut self, peer_ids: Vec<PeerId>, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        let group_number = my_open_mls.create_mls_group(&peer_ids)?;
        let group_id = my_open_mls.get_groups().get(group_number).unwrap().clone();
        //Subscribes to partitionedchannel and adds groupid to topic list
        if let Err(err) = self.subscribe(group_id.to_vec(), my_open_mls){
            return Err(err);
        }


        Ok(())
    }

    // List all MLS groups
    fn list_groups(&mut self, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError>{
        for (index, group_id) in my_open_mls.get_groups().iter().enumerate() {
            println!("[{index}]: {group_id:?}");
        }
        Ok(())
    }

    // Add new Group Member to MLS Group
    fn add_group_member(&mut self, my_open_mls: &MyOpenMls, group_number: usize, peer_id: PeerId) -> Result<(), AbstSwarmError>{
        let (group_id, mls_message_out, epoch) = my_open_mls.propose_add_group_member(group_number, peer_id)?;
        Ok(self.send_proposal(my_open_mls, group_id, mls_message_out, epoch)?)
    }

    // Remove Group Member from MLS Group
    fn remove_group_member(&mut self, my_open_mls: &MyOpenMls, group_number: usize, peer_id: PeerId) -> Result<(), AbstSwarmError>{
        let (group_id, mls_message_out,epoch) = my_open_mls.propose_remove_group_member(group_number, peer_id)?;
        self.send_proposal(my_open_mls, group_id, mls_message_out,epoch)
    }

    // Deleve Group for all members(including owner)
    fn delete_group_for_all(&mut self, my_open_mls: &MyOpenMls, group_number: usize)-> Result<(), AbstSwarmError>{
        let group_id = my_open_mls.delete_group_for_all(group_number)?;
        my_open_mls.remove_mls_group(group_id.clone());
        self.unsubscribe(group_id,my_open_mls)
    }

    // Send a new group proposal
    fn send_proposal(&mut self, my_open_mls: &MyOpenMls, group_id: Vec<u8>, data: Vec<u8>, epoch:u64) -> Result<(), AbstSwarmError>{
        let body = MessageBody::Proposal {group_id: group_id.clone(), data, epoch};
        self.send(body, my_open_mls)
    }

    // Update Key material for group
    fn update_key_material(&mut self, my_open_mls: &MyOpenMls, group_number: usize) -> Result<(), AbstSwarmError>{
        let (group_id, mls_message_out,epoch) = my_open_mls.update_key_material(group_number)?;
        self.send_proposal(my_open_mls, group_id, mls_message_out,epoch)
    }

    // Request Group Updates from an Archive Peer
    fn update_groups_req(&mut self,my_open_mls: &MyOpenMls, peer_id: PeerId) -> Result<(), AbstSwarmError>{
        for group_id in my_open_mls.get_groups(){
            let epoch = my_open_mls.get_group_epoch(&group_id).ok_or(GroupNotAvailable("Could not find group mentioned in update req".to_string()))?;
            self.request_response.send_request(&peer_id,my_open_mls.sign(RequestMessage::Update {group_id: group_id.to_vec(), epoch})?);
        }
        Ok(())
    }

    // Register new identity at authority
    //This Function does not sign the Request, Since no Keys are available
    fn register_identity(&mut self, peer_id: PeerId, identity: String) -> Result<(), AbstSwarmError>{
        self.request_response.send_request(&peer_id,Request{body:RequestMessage::VerifyIdentity {identity, with_signature:true},signature:None});
        Ok(())
    }

    // Register this Peer at authority with existing identity
    fn peer_from_sig(&mut self, my_open_mls: &MyOpenMls ,peer_id: PeerId, sig_file_path: String) -> Result<(), AbstSwarmError>{
        let path = PathBuf::from_str(&sig_file_path).map_err(|_|KeyMaterialFileError)?;
        let new_key_store = key_store::StoredKeyMaterial::from_file(path).map_err(|_|KeyMaterialFileError)?;
        my_open_mls.set_grpkey(new_key_store.get_grpkey().ok_or(KeyMaterialFileError)?.to_bytes())?;
        my_open_mls.set_memkey(new_key_store.get_memkey().ok_or(KeyMaterialFileError)?.to_bytes())?;
        my_open_mls.set_id(new_key_store.get_id().ok_or(KeyMaterialFileError)?)?;

        let (enc_shares, commits) = my_open_mls.generate_enc_shares()?;

        let keypackage = my_open_mls.generate_key_package()?;
        let keypackage = bincode::serialize(&keypackage)
            .map_err(|x|SerializationError(x.to_string()))?;
        let body = RequestMessage::AddPeerToSig {keypackage,enc_shares,commits};
        self.request_response.send_request(&peer_id,my_open_mls.sign(body)?);
        Ok(())
    }
    
    fn get_opener_keys(&mut self ,peer_id: PeerId) -> Result<(), AbstSwarmError>{
        let request= Request{body:RequestMessage::RequestOpenerKeys, signature:None};
        self.request_response.send_request(&peer_id,request);
        Ok(())
    }

    // Group Owner publishes commit with all
    fn publish_commit(&mut self, my_open_mls: &MyOpenMls, group_id: Vec<u8>, peer_ids: Vec<PeerId>,commit: Vec<u8>, welcome_option: Option<Vec<u8>>, epoch: u64) -> Result<(), AbstSwarmError> {
        my_open_mls.store_to_file()?;
        let body =  MessageBody::Commit {group_id: group_id.clone(), commit, welcome_option:welcome_option.clone(), epoch};
        if let Some(welcome) = welcome_option.clone(){
            self.send_out_welcomes(my_open_mls,peer_ids, welcome,my_open_mls.get_ratchet_tree(&group_id)?)?;
        }
        self.send(body, my_open_mls)
    }

    /*
    This filters and sorts the update response of an archive to make sure that commits and proposals are added in the right order
     */
    fn filter_and_group_messages(messages: Vec<MessageBody>, general_group_id: Vec<u8>) -> Result<Vec<(Vec<MessageBody>, MessageBody)>, AbstSwarmError> {
        //The following error handling is only helpful for a benign archive-response.
        //Malicious archives could manipulate the outer epochs to not match the epochs in the messages.
        //Such errors are detected by MLS itself.

        // Separate proposals and commits
        let mut proposals: Vec<MessageBody> = Vec::new();
        let mut commits: Vec<MessageBody> = Vec::new();

        for message in messages {
            match message {
                MessageBody::Proposal {..} => proposals.push(message),
                MessageBody::Commit {..} => commits.push(message),
                _ => {}
            }
        }

        // Group proposals and commits by epoch using a BTreeMap
        let mut proposals_by_epoch: BTreeMap<u64, Vec<MessageBody>> = BTreeMap::new();
        let mut commits_by_epoch: BTreeMap<u64, MessageBody> = BTreeMap::new();

        for proposal in proposals {
            if let MessageBody::Proposal { epoch, group_id, .. } = proposal.clone() {
                if general_group_id == group_id {
                    proposals_by_epoch.entry(epoch).or_insert_with(Vec::new).push(proposal);
                }
            }
        }

        for commit in commits {
            if let MessageBody::Commit { epoch, group_id, .. } = commit.clone() {
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
                    return Err(HandleUpdateError {reason:"Commit epochs not in strictly rising order".to_string()})
                }
            }

            if let Some(proposals) = proposals_by_epoch.get(&(epoch - 1)) {
                result.push(( proposals.clone(), commit));

            } else {
                result.push(( Vec::new(), commit));
            }
            last_epoch = Some(epoch);
        }

        Ok(result)
    }

    // handle the Group update response from an archive
    fn handle_update(&mut self, my_open_mls: &MyOpenMls,group_id: Vec<u8>, messages: Vec<MessageBody>) -> Result<(), AbstSwarmError>{
        let filtered = Self::filter_and_group_messages(messages ,group_id.clone())?;

        for (proposals, commit) in filtered{
            //Insert all missed proposals into proposal store
            for proposal in proposals {
                if let MessageBody::Proposal {data,..} = proposal {
                    my_open_mls.store_proposal(group_id.clone(), data).map_err(|e| HandleUpdateError {reason: e.to_string()})?;
                }

            }
            //Commit
            if let MessageBody::Commit {group_id: commit_group_id, commit: commit_vec, welcome_option,..} = commit {
                if self.handle_commit(my_open_mls, commit_group_id, commit_vec, welcome_option)?{
                    return Ok(())
                }
            }

        }
        Ok(())
    }

    //Handles Commit returns true if self has been removed in commit
    fn handle_commit(&mut self, my_open_mls: &MyOpenMls,group_id: Vec<u8>, commit: Vec<u8>, welcome_option: Option<Vec<u8>>) -> Result<bool,AbstSwarmError> {
        let (peer_ids, ratchet_tree, self_removed) = match my_open_mls.handle_commit(group_id.clone(),commit) {
            Ok(val) => val,
            Err(GroupNotAvailable(_)) => {
                //Ignore message for groups, that I do not have
                return Ok(false);
            },
            Err(e) => return Err(MLSError(e)),
        };
        if !peer_ids.is_empty(){
            log::info!("Proposed Members have been added to group {group_id:?}, sending out welcome messages");
            if let Some(welcome) = welcome_option {
                self.send_out_welcomes(my_open_mls,peer_ids,welcome,ratchet_tree)?
            }
        }
        if self_removed{
            log::info!("Removed from Group {:?}, via commit. \n Deleting Group",group_id);
            my_open_mls.remove_mls_group(group_id.clone());
            self.unsubscribe(group_id.clone(),my_open_mls).map_err(|e| HandleUpdateError {reason: e.to_string()})?;
        }
        my_open_mls.store_to_file()?;
        return Ok(self_removed)
    }

    fn handle_pubsub_event(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        event: gossipsub::Event,
    ) -> Result<(),AbstSwarmError>{
        match event {
            gossipsub::Event::Message {
                propagation_source: peer_id,
                message_id: id,
                message,
            } => {
                let message: Message= bincode::deserialize(&message.data).map_err(|e|SerializationError(e.to_string()))?;
                let (message_body, signature) = match my_open_mls.verify(message) {
                    Ok(message) => {
                        self.gossipsub.report_message_validation_result(&id, &peer_id, MessageAcceptance::Accept).map_err(|_| MessageValidationError)?;
                        message
                    },
                    Err(e) => return match e {
                            MyOpenMLSError::SignatureInvalid {signature: mysig} => {
                                log::debug!("Got a Message with Invalid Signature: {}", hex::encode(&mysig.to_bytes()));
                                self.gossipsub.report_message_validation_result(&id, &peer_id, MessageAcceptance::Reject).map_err(|_| MessageValidationError)?;
                                Ok(())
                            }
                            e => Err(MLSError(e))

                    },
                };


                match message_body {
                    MessageBody::GroupEnc {group_id, enc_data: data, nonce} => {
                        //Validate Group Signature
                        let decrypt_res= my_open_mls.decrypt_with_group_key(group_id, data, nonce);
                        match decrypt_res {
                            Ok((group_number_option, data)) => {
                                let group_number = group_number_option.ok_or(GroupNotAvailable("".to_string()))?;
                                let message_body = MessageBody::Group {group_number, data};
                                Self::notify(notification_tx, Notification::Data{propagation_source:peer_id, message_body, signature:signature.to_bytes()})
                            }
                            Err(e) => match e {
                                GroupNotAvailable(_) => {}, //Do nothing, because not my group
                                MyOpenMLSError::SignatureInvalid{signature} => log::error!("Received message with invalid Signature: {}", hex::encode(signature.to_bytes())),
                                SerializationError(e) => log::error!("Received message but could not decode: {e}"),
                                _ => log::error!("Received a gossipsub message, which caused an unhandeled error: {e}"),
                            }
                        }
                    },

                    MessageBody::Broadcast { .. } => {},  // Peers should only receive encrypted messages
                    MessageBody::BroadcastEnc {topic, data, nonce} => { // Handle public message

                        if my_open_mls.subscibed_to_topic(topic.as_bytes().to_vec()) {
                            let data= my_open_mls.decrypt_broadcast(data,nonce)?;
                            let message_body = MessageBody::Broadcast {topic, data};
                            Self::notify(notification_tx, Notification::Data{propagation_source:peer_id, message_body:message_body.clone(),signature:signature.to_bytes()})
                        }
                    }
                    MessageBody::Group { .. } => { } // Peers should only receive encrypted messages // Handle private message
                    MessageBody::Proposal {group_id,data,..} => {   // Handle group proposal
                        log::info!("Got a Proposal for {group_id:?}");
                        if let Err(e) = my_open_mls.store_proposal(group_id, data){
                            match e {
                                GroupNotAvailable(_) => { /* Ignore Groups that I do not belong */ },
                                _ => return Err(MLSError(e)),
                            }
                        }
                    },
                    MessageBody::Commit {group_id,commit, welcome_option, ..} => { // Handle group commit
                        log::info!("Got a Commit for {group_id:?}");
                        self.handle_commit(my_open_mls, group_id, commit, welcome_option)?;
                    }
                }

            },
            _ => log::debug!("Received Pubsub event:  {event:?}"),
        }

        Ok(())
    }


    // Send out welcome message to peers added to a group
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

    /// Key Package Requests, Identity Registration and Archive Updates
    fn handle_request_response_event(&mut self, request_event: request_response::Event<Request,Response>, my_open_mls: &MyOpenMls) -> Result<(),AbstSwarmError>{
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
                        let (request_body,_) = match my_open_mls.verify(request) {
                            Ok(message) => {
                                message
                            },
                            Err(e) => return match e {
                                MyOpenMLSError::SignatureInvalid {signature: mysig} => {
                                    log::debug!("Got a Request with Invalid Signature: {}", hex::encode(&mysig.to_bytes()));
                                    Ok(())
                                }
                                e => Err(MLSError(e))

                            },
                        };
                        match request_body {
                            RequestMessage::KeyPackage => {
                                log::info!("Received KeyPackage request from Peer {}", peer.to_string());
                                let keypackage = my_open_mls.generate_key_package()?;
                                self.request_response.send_response(channel, my_open_mls.sign(ResponseMessage::KeyPackage(bincode::serialize(&keypackage)
                                    .map_err(|x|SerializationError(x.to_string()))?))?)
                                    .map_err(|_| SendKeyPackageError)?
                            }
                            RequestMessage::JoinGroup { welcome, ratchet_tree } => {
                                log::info!("Received a JoinGroup Request from Peer {}", peer.to_string());
                                let group_id = my_open_mls.add_group_from_welcome(welcome, ratchet_tree)?;
                                //If from authority it is the common group
                                if my_open_mls.get_authorityid() == peer {
                                    my_open_mls.move_to_common_id(group_id.clone())?;
                                }
                                log::info!("Added Group {group_id:?}");
                                self.subscribe(group_id.clone(), my_open_mls)?;
                                self.request_response.send_response(channel, my_open_mls.sign(ResponseMessage::JoinGroup)?).map_err(|_| SendJoinGroupError {group_id})?
                            }
                            _ => {}
                        }
                    }
                    //It is not checked if a response belongs to a send request, this could cause security issues
                    request_response::Message::Response {
                        response,..
                    } => {
                        match response.body {
                            ResponseMessage::KeyPackage(ref val) => {
                                //Only Verify KPs
                                if let Err(e) = my_open_mls.verify(response.clone()) {
                                    return match e {
                                        MyOpenMLSError::SignatureInvalid { signature: mysig } => {
                                            log::debug!("Got a Response with Invalid Signature: {}", hex::encode(&mysig.to_bytes()));
                                            Ok(())
                                        }
                                        e => Err(MLSError(e))
                                    }
                                };
                                log::info!("Response with KeyPackage from {}", peer.to_string());
                                let keypackage: KeyPackage = bincode::deserialize(val).map_err(|x| SerializationError(x.to_string()))?;
                                my_open_mls.insert_foreign_kp(peer, keypackage)
                            }
                            ResponseMessage::JoinGroup => {
                                //Nothing to do for now
                            }
                            ResponseMessage::AddPeerShare => {
                                //Nothing to do for now
                            }
                            ResponseMessage::EncSharePairings {..} => {
                                //Nothing to do for now
                            }
                            ResponseMessage::ResUpdate {group_id,messages} => {
                                log::info!("Got an update for group ´{group_id:?}´ with {} messages", messages.len());
                                self.handle_update(my_open_mls,group_id,messages)?;
                            }
                            ResponseMessage::IdentityVerified {id, n, group_key, opener_pubkeys} => {
                                log::info!("Identity was verified, proceeding with memkey creation.");
                                let n = match n {
                                    Some(val) => val,
                                    None => return Ok(())
                                };
                                let opener_pubkeys = match opener_pubkeys {
                                    Some(val) => val,
                                    None => return Ok(())
                                };
                                my_open_mls.add_opener_pubkeys(&opener_pubkeys)?;
                                let (g_sk, h_sk, pi1,pi2,enc_shares,commits) = my_open_mls.generate_join_key_material(id, n,group_key)?;
                                my_open_mls.set_id(id)?;
                                let keypackage = my_open_mls.generate_key_package()?;
                                let keypackage = bincode::serialize(&keypackage)
                                    .map_err(|x|SerializationError(x.to_string()))?;
                                self.request_response.send_request(&peer, Request{body:RequestMessage::JoinKeyMaterial {g_sk,h_sk,pi1,pi2,enc_shares,commits, keypackage}, signature:None});

                            }
                            ResponseMessage::MemKey {partial_memkey} => {
                                if let Some(memkey) = partial_memkey{
                                    my_open_mls.add_partial_memkey(memkey)?;
                                }
                            }
                            ResponseMessage::PeerAdded => {  }
                            ResponseMessage::OpenerKeys {opener_pubkeys} => {
                                log::info!("Got Opener Keys.");
                                my_open_mls.add_opener_pubkeys(&opener_pubkeys)?;
                            }
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
impl InstructionHandler for MyBehaviour {
    async fn handle_instruction(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: & MyOpenMls,
        instruction: Instruction,
    ) {
        match instruction {
            Instruction::Send {
                message,
            } => {
                if let Err(error) = self.send(message, my_open_mls) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            },
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
                if let Err(error) = self.get_keypackages(my_open_mls,peer_ids) {
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
            Instruction::AddGroupMember {group_number,peer_id} => {
                if let Err(error) = self.add_group_member(my_open_mls, group_number, peer_id) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::RemoveGroupMember {group_number, peer_id} => {
                if let Err(error) = self.remove_group_member(my_open_mls, group_number, peer_id) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::DeleteGroupForAll {group_number} => {
                if let Err(error) = self.delete_group_for_all(my_open_mls, group_number) {
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::UpdateKeyMaterial {group_number} => {
                if let Err(error) = self.update_key_material(my_open_mls, group_number){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::Commit {group_id,peer_ids, commit, welcome, epoch} => {
                if let Err(error) = self.publish_commit(my_open_mls, group_id.to_vec(),peer_ids, commit, welcome, epoch){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::UpdateGroups {peer_id} => {
                if let Err(error) = self.update_groups_req(my_open_mls,peer_id){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::RegisterIdentity {peer_id, identity} => {
                if let Err(error) = self.register_identity(peer_id,identity){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::PeerFromSig {peer_id, sig_file_path} => {
                if let Err(error) = self.peer_from_sig(my_open_mls,peer_id,sig_file_path){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::GetOpenerKeys {peer_id} => {
                if let Err(error) = self.get_opener_keys(peer_id){
                    Self::notify(notification_tx, Notification::Err(error));
                }
            }
            Instruction::ReqOpen {..} => {} //Ignore
            Instruction::Open {..} => {} //Ignore
            Instruction::EndInitArchive {..} => {} //Ignore

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
        if let Err(e) = match event {
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(event)) => {
                self.handle_pubsub_event(notification_tx, my_open_mls,event)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::IdentifyEvent(e)) => {
                self.handle_identify_event(e)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::RequestResponseEvent(e)) => {
                self.handle_request_response_event(e,my_open_mls)
            }
            non_functional_event => self.handle_non_functional_event(non_functional_event),

        } {
            Self::notify_error(notification_tx, e);
        }
    }
}



#[derive(Debug)]
pub(crate) enum MyBehaviourEvent {
    Gossipsub(gossipsub::Event),
    KademliaEvent(()),
    IdentifyEvent(Box<identify::Event>),
    RequestResponseEvent(request_response::Event<Request,Response>),
}

impl From<gossipsub::Event> for MyBehaviourEvent {
    fn from(event: gossipsub::Event) -> Self {
        MyBehaviourEvent::Gossipsub(event)
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

