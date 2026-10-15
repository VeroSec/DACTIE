use crate::mybehaviour::MyBehaviourError::PubSubBehaviourError;
use async_trait::async_trait;
use dactie_utils::net::{is_unroutable_addr, MESH_N, MESH_N_HIGH, MESH_N_LOW};
use dactie_utils::admission;
use dactie_utils::shared_structs::*;
use libp2p::futures::TryStreamExt;
use libp2p::gossipsub::{IdentTopic, MessageAcceptance};
use libp2p::identity::{Keypair, ParseError};
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{Mode, PROTOCOL_NAME};
use libp2p::request_response::ProtocolSupport;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identify, kad, request_response, Multiaddr, StreamProtocol};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::sync::OnceLock;
use std::time::Duration;
use thiserror::Error;
use tokio::io;

use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::{AbstSwarmError, PeerId};

use crate::abstswarm::instruction_handler::InstructionHandler;
use crate::abstswarm::AbstSwarmError::{
    MLSError, MessageValidationError, SendJoinGroupError, SendResponseError,
};
use dactie_utils::mls_wrapper::MyOpenMLSError::{GroupNotAvailable, SerializationError};
use dactie_utils::mls_wrapper::{MyOpenMLSError, MyOpenMls};

const NUM_CHANNELS: u64 = 10;

/// Whether opening requests from the authority are answered without operator approval.
/// Off by default: the threshold of archives only protects anonymity if each archive decides.
pub(crate) static AUTO_APPROVE_OPEN: OnceLock<bool> = OnceLock::new();

/// Short, stable handle for a pending opening request, shown to the operator.
fn open_request_id(signature: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    signature.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Pending opening requests outlive restarts, so an operator can decide later.
pub(crate) async fn ensure_pending_opens_table(pool: &sqlx::PgPool) -> Result<(), MyBehaviourError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS pending_opens (
            id TEXT PRIMARY KEY,
            signature BYTEA NOT NULL,
            received_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
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
    /// this is used for the responding to peers update requests
    pub(crate) request_response: request_response::json::Behaviour<Request, Response>,
    /// Limits connections of nodes outside the common MLS group.
    pub(crate) gate: admission::Gate,
}

#[derive(sqlx::FromRow, Debug)]
struct Commit {
    group_id: Vec<u8>,
    commit: Vec<u8>,
    welcome_option: Option<Vec<u8>>,
    epoch: i64,
}

#[derive(sqlx::FromRow, Debug)]
struct Proposal {
    group_id: Vec<u8>,
    data: Vec<u8>,
    epoch: i64,
}

#[derive(sqlx::FromRow, Debug)]
struct IDShare {
    id: i64,
    enc_share: Vec<u8>,
}

#[derive(sqlx::FromRow, Debug)]
struct PeerShare {
    peer_id: Vec<u8>,
    enc_share: Vec<u8>,
}

impl MyBehaviour {
    pub(crate) fn new(
        keypair: &Keypair,
        boot_peer_id: PeerId,
        boot_peer_addr: Multiaddr,
    ) -> Result<MyBehaviour, MyBehaviourError> {
        // To content-address message, we can take the hash of message and use it as an ID.
        let message_id_fn = |message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        };

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(1)) // This is set to aid debugging by not cluttering the log space
            .validation_mode(gossipsub::ValidationMode::Permissive) //Allows anonymous peers
            .validate_messages() //Used to manually verify group signatures for each message
            // Real CTI events are far larger than gossipsub's 65536-byte default,
            // which a 64KiB MISP event already exceeds once the group signature
            // and framing are added. Raised so the evaluation can carry actual
            // MISP payloads; every peer must agree on this or large events are
            // dropped on receipt.
            .max_transmit_size(8 * 1024 * 1024)
            .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
            .mesh_n_low(MESH_N_LOW)
            .mesh_n(MESH_N)
            .mesh_n_high(MESH_N_HIGH)
            .fanout_ttl(admission::FANOUT_TTL)
            .build()
            .map_err(io::Error::other)
            .unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.

        let mut gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .map_err(|err| PubSubBehaviourError(err.to_string()))?;
        admission::enable(&mut gossipsub).map_err(PubSubBehaviourError)?;

        gossipsub
            .subscribe(&IdentTopic::new("Test"))
            .map_err(|err| PubSubBehaviourError(err.to_string()))?;

        //Subscribe to Partitioned Channels
        for i in 0..NUM_CHANNELS {
            gossipsub
                .subscribe(&IdentTopic::new(format!("channel_{i}")))
                .map_err(|err| PubSubBehaviourError(err.to_string()))?;
        }

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
        // Add the bootnodes to the local routing table.
        kademlia.add_address(&boot_peer_id, boot_peer_addr);

        let request_response = request_response::json::Behaviour::new(
            iter::once((StreamProtocol::new("/key_package/1"), ProtocolSupport::Full)),
            request_response::Config::default(),
        );

        Ok(Self {
            gossipsub,
            identify,
            kademlia,
            request_response,
            gate: admission::Gate::default(),
        })
    }

    /**
    Function used to insert messages into the DB
     **/
    async fn insert_broadcast(
        pool: &sqlx::PgPool,
        id: &[u8],
        channel: &str,
        topic: &str,
        data: &[u8],
    ) -> Result<(), MyBehaviourError> {
        let result = sqlx::query(
            "INSERT INTO broadcast (id, channel, topic, data) VALUES ($1, $2, $3, $4)",
        )
            .bind(id)
            .bind(channel.to_string())
            .bind(topic.to_string())
            .bind(data)
            .execute(pool)
            .await?;
        log::debug!("{result:?}");

        Ok(())
    }

    async fn insert_group_enc(
        pool: &sqlx::PgPool,
        id: &[u8],
        group_id: &[u8],
        data: &[u8],
        nonce: &[u8],
    ) -> Result<(), MyBehaviourError> {
        let result = sqlx::query(
            "INSERT INTO group_enc (id, group_id, data, nonce) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(group_id)
        .bind(data)
        .bind(nonce)
        .execute(pool)
        .await?;
        log::debug!("{result:?}");
        Ok(())
    }

    async fn insert_proposal(
        pool: &sqlx::PgPool,
        id: &[u8],
        group_id: &[u8],
        data: &[u8],
        epoch: u64,
    ) -> Result<(), MyBehaviourError> {
        let result =
            sqlx::query("INSERT INTO proposal (id, group_id, data, epoch) VALUES ($1, $2, $3, $4)")
                .bind(id)
                .bind(group_id)
                .bind(data)
                .bind(epoch as i64) //SQLX and probably Postgres do not support u64. Care has to be taken, when retrieving this value from the database
                .execute(pool)
                .await?;
        log::debug!("{result:?}");
        Ok(())
    }

    async fn insert_commit(
        pool: &sqlx::PgPool,
        id: &[u8],
        group_id: &[u8],
        commit: &[u8],
        welcome_option: Option<&[u8]>,
        epoch: u64,
    ) -> Result<(), MyBehaviourError> {
        let result = sqlx::query(
            "INSERT INTO commit (id, group_id, commit, welcome_option, epoch) VALUES ($1, $2, $3, $4, $5)")
            .bind(id)
            .bind(group_id)
            .bind(commit)
            .bind(welcome_option)
            .bind(epoch as i64)         //SQLX and probably Postgres do not support u64. Care has to be taken, when retrieving this value from the database
            .execute(pool).await?;
        log::debug!("{result:?}");
        Ok(())
    }

    async fn insert_id_enc_share(
        pool: &sqlx::PgPool,
        id: usize,
        enc_share: &[u8],
        commits: &[u8],
    ) -> Result<(), MyBehaviourError> {
        let result =
            sqlx::query("INSERT INTO id_shares (id, enc_share, commits) VALUES ($1, $2, $3)")
                .bind(id as i64) //SQLX and probably Postgres do not support usize. Care has to be taken, when retrieving this value from the database
                .bind(enc_share)
                .bind(commits)
                .execute(pool)
                .await?;
        log::debug!("{result:?}");
        Ok(())
    }

    async fn insert_peer_enc_share(
        pool: &sqlx::PgPool,
        peer_id: &[u8],
        enc_share: &[u8],
        commits: &[u8],
    ) -> Result<(), MyBehaviourError> {
        let result = sqlx::query(
            "INSERT INTO peer_shares (peer_id, enc_share, commits) VALUES ($1, $2, $3)",
        )
        .bind(peer_id) //SQLX and probably Postgres do not support usize. Care has to be taken, when retrieving this value from the database
        .bind(enc_share)
        .bind(commits)
        .execute(pool)
        .await?;
        log::debug!("{result:?}");
        Ok(())
    }

    /**
    Functions that return DB Content to the peers
    **/
    async fn get_newer_commits(
        pool: &sqlx::PgPool,
        group_id: &[u8],
        epoch: u64,
    ) -> Result<Vec<MessageBody>, MyBehaviourError> {
        let mut commit_stream= sqlx::query_as::<_, Commit>(
        "SELECT id, group_id, commit, welcome_option, epoch FROM commit WHERE group_id = $1 AND epoch > $2")
            .bind(group_id)
            .bind(epoch as i64)
            .fetch(pool);

        let mut commits = Vec::new();
        while let Some(Commit {
            group_id,
            commit,
            welcome_option,
            epoch,
            ..
        }) = commit_stream.try_next().await?
        {
            commits.push(MessageBody::Commit {
                group_id,
                commit,
                welcome_option,
                epoch: epoch as u64,
            })
        }
        Ok(commits)
    }

    async fn get_newer_proposals(
        pool: &sqlx::PgPool,
        group_id: &[u8],
        epoch: u64,
    ) -> Result<Vec<MessageBody>, MyBehaviourError> {
        let mut proposal_stream = sqlx::query_as::<_, Proposal>(
            "SELECT id, group_id, data, epoch FROM proposal WHERE group_id = $1 AND epoch >= $2",
        )
        .bind(group_id)
        .bind(epoch as i64)
        .fetch(pool);

        let mut proposals = Vec::new();
        while let Some(Proposal {
            group_id,
            data,
            epoch,
            ..
        }) = proposal_stream.try_next().await?
        {
            proposals.push(MessageBody::Proposal {
                group_id,
                data,
                epoch: epoch as u64,
            })
        }
        Ok(proposals)
    }

    async fn get_all_id_enc_shares(
        pool: &sqlx::PgPool,
    ) -> Result<Vec<(usize, Vec<u8>)>, MyBehaviourError> {
        let mut proposal_stream =
            sqlx::query_as::<_, IDShare>("SELECT id, enc_share FROM id_shares").fetch(pool);

        let mut shares = Vec::new();
        while let Some(IDShare { id, enc_share }) = proposal_stream.try_next().await? {
            shares.push((id as usize, enc_share));
        }
        Ok(shares)
    }

    async fn get_all_peer_enc_shares(
        pool: &sqlx::PgPool,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, MyBehaviourError> {
        let mut proposal_stream =
            sqlx::query_as::<_, PeerShare>("SELECT peer_id, enc_share FROM peer_shares")
                .fetch(pool);

        let mut shares = Vec::new();
        while let Some(PeerShare { peer_id, enc_share }) = proposal_stream.try_next().await? {
            shares.push((peer_id, enc_share));
        }
        Ok(shares)
    }

    /**
        Function that register the identity of the archive and handle common group commits.
    **/
    async fn queue_open(
        pool: &sqlx::PgPool,
        id: &str,
        signature: &[u8],
    ) -> Result<(), MyBehaviourError> {
        sqlx::query(
            "INSERT INTO pending_opens (id, signature) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(signature)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn take_pending_open(
        pool: &sqlx::PgPool,
        id: &str,
    ) -> Result<Option<Vec<u8>>, MyBehaviourError> {
        let row: Option<(Vec<u8>,)> =
            sqlx::query_as("DELETE FROM pending_opens WHERE id = $1 RETURNING signature")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|(signature,)| signature))
    }

    async fn print_pending_opens(&self, pool: &sqlx::PgPool) -> Result<(), AbstSwarmError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, to_char(received_at, 'YYYY-MM-DD HH24:MI:SS') FROM pending_opens ORDER BY received_at",
        )
        .fetch_all(pool)
        .await
        .map_err(MyBehaviourError::from)?;
        if rows.is_empty() {
            println!("No pending opening requests");
        }
        for (id, received_at) in rows {
            println!("{id}  received {received_at}");
        }
        Ok(())
    }

    async fn approve_open(
        &mut self,
        pool: &sqlx::PgPool,
        my_open_mls: &MyOpenMls,
        id: &str,
    ) -> Result<(), AbstSwarmError> {
        match Self::take_pending_open(pool, id).await? {
            Some(signature) => {
                self.submit_open_pairings(pool, my_open_mls, signature).await?;
                println!("Submitted this archive's partial opening for {id}");
            }
            None => println!("No pending opening request {id}"),
        }
        Ok(())
    }

    async fn deny_open(&self, pool: &sqlx::PgPool, id: &str) -> Result<(), AbstSwarmError> {
        match Self::take_pending_open(pool, id).await? {
            Some(_) => println!("Denied opening request {id}"),
            None => println!("No pending opening request {id}"),
        }
        Ok(())
    }

    /// Computes this archive's partial opening over every stored share, since the signer is
    /// unknown, and hands it to the authority, which combines the partial openings.
    async fn submit_open_pairings(
        &mut self,
        pool: &sqlx::PgPool,
        my_open_mls: &MyOpenMls,
        signature: Vec<u8>,
    ) -> Result<(), AbstSwarmError> {
        let id_enc_shares = Self::get_all_id_enc_shares(pool).await?;
        let peer_enc_shares = Self::get_all_peer_enc_shares(pool).await?;
        let enc_share_id_pairings =
            bincode::serialize(&my_open_mls.get_enc_share_pairings(id_enc_shares, &signature)?)
                .map_err(|e| SerializationError(e.to_string()))?;
        let enc_share_peer_pairings =
            bincode::serialize(&my_open_mls.get_enc_share_pairings(peer_enc_shares, &signature)?)
                .map_err(|e| SerializationError(e.to_string()))?;
        self.request_response.send_request(
            &my_open_mls.get_authorityid(),
            Request {
                body: RequestMessage::SubmitOpenPairings {
                    signature,
                    enc_share_id_pairings,
                    enc_share_peer_pairings,
                },
                signature: None,
            },
        );
        Ok(())
    }

    fn register_identity(&mut self, peer_id: &PeerId, identity: String) {
        let request = Request {
            body: RequestMessage::VerifyIdentity {
                identity,
                with_signature: false,
            },
            signature: None,
        };
        self.request_response.send_request(peer_id, request);
    }

    fn handle_commit(
        &mut self,
        my_open_mls: &MyOpenMls,
        group_id: Vec<u8>,
        commit: Vec<u8>,
    ) -> Result<bool, AbstSwarmError> {
        let (_, _, self_removed) = match my_open_mls.handle_commit(group_id.clone(), commit) {
            Ok(val) => val,
            Err(GroupNotAvailable(_)) => {
                //Ignore message for groups, that I do not have
                return Ok(false);
            }
            Err(e) => return Err(MLSError(e)),
        };
        if self_removed {
            log::info!(
                "Removed from Group {:?}, via commit. \n Deleting Group",
                group_id
            );
            my_open_mls.remove_mls_group(group_id.clone());
            self.unsubscribe(group_id.clone(), my_open_mls)?;
        }
        my_open_mls.store_to_file()?;
        Ok(self_removed)
    }

    /**
    Functions that handle gossipsub behaviour and un-subscribing to all channels.
    **/
    fn subscribe(&mut self, topic: Vec<u8>, my_open_mls: &MyOpenMls) -> Result<(), AbstSwarmError> {
        if let Err(err) = my_open_mls.subscribe(topic.clone()) {
            return Err(MLSError(err));
        }
        my_open_mls.store_to_file()?;
        Ok(())
    }

    fn unsubscribe(
        &mut self,
        topic: Vec<u8>,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        if let Err(err) = my_open_mls.unsubscribe(topic) {
            return Err(MLSError(err));
        }
        my_open_mls.store_to_file()?;
        Ok(())
    }

    async fn handle_pubsub_event(
        &mut self,
        event: gossipsub::Event,
        pool: &sqlx::PgPool,
        my_open_mls: &MyOpenMls,
    ) -> Result<(), AbstSwarmError> {
        match event {
            gossipsub::Event::Message {
                propagation_source: peer_id,
                message_id: id,
                message,
            } => {
                let arrived_on = message.topic.clone();
                let message: Message = bincode::deserialize(&message.data)
                    .map_err(|e| SerializationError(e.to_string()))?;
                // A broadcast replayed on another channel than the one its signed
                // body names is rejected and not forwarded.
                if let MessageBody::BroadcastEnc { channel, .. } = &message.body {
                    if IdentTopic::new(channel.clone()).hash() != arrived_on {
                        log::warn!("Rejecting a broadcast for {channel} that arrived on {arrived_on} from {peer_id}");
                        self.gossipsub
                            .report_message_validation_result(&id, &peer_id, MessageAcceptance::Reject)
                            .map_err(|_| MessageValidationError)?;
                        return Ok(());
                    }
                }
                let (message_body, _) = match my_open_mls.verify(message) {
                    Ok(message) => {
                        self.gossipsub
                            .report_message_validation_result(
                                &id,
                                &peer_id,
                                MessageAcceptance::Accept,
                            )
                            .map_err(|_| MessageValidationError)?;
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

                match message_body {
                    MessageBody::BroadcastEnc { channel, data, nonce } => {
                        // Archives are members of the common group, so they can
                        // read the topic; peers query them by channel only.
                        match my_open_mls.open_broadcast(&channel, data, nonce) {
                            Ok((topic, plaintext)) => {
                                Self::insert_broadcast(pool, &id.0, &channel, &topic, &plaintext).await?;
                                println!("Got a Broadcast");
                            }
                            Err(e @ MyOpenMLSError::ChannelMismatch { .. }) => {
                                log::warn!("Not storing broadcast from {peer_id}: {e}")
                            }
                            Err(e) => return Err(MLSError(e)),
                        }
                    }
                    MessageBody::GroupEnc {
                        group_id,
                        enc_data: data,
                        nonce,
                    } => {
                        Self::insert_group_enc(pool, &id.0, &group_id, &data, &nonce).await?;
                        println!("Got a Group_Enc");
                    }
                    MessageBody::Proposal {
                        group_id,
                        data,
                        epoch,
                    } => {
                        Self::insert_proposal(pool, &id.0, &group_id, &data, epoch).await?;
                        println!("Got a Proposal");
                    }
                    MessageBody::Commit {
                        group_id,
                        commit,
                        welcome_option,
                        epoch,
                    } => {
                        Self::insert_commit(
                            pool,
                            &id.0,
                            &group_id,
                            &commit,
                            welcome_option.as_deref(),
                            epoch,
                        )
                        .await?;
                        self.handle_commit(my_open_mls, group_id, commit)?;
                        println!("Got a Commit");
                    }
                    MessageBody::Group { .. } | MessageBody::Broadcast { .. } => {
                        println!("Got something that I was not supposed to get")
                    }
                }
            }
            _ => log::debug!("Received Pubsub event:  {event:?}"),
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
            if protocols.contains(&PROTOCOL_NAME) {
                for addr in listen_addrs {
                    // Skip addresses no remote host can dial (loopback, Docker bridges).
                    if is_unroutable_addr(&addr) {
                        log::debug!("Ignoring unroutable addr from identify for {}: {}", peer_id, addr);
                        continue;
                    }
                    log::debug!("Adding received IdentifyInfo matching protocol to the DHT. Peer: {}, addr: {}", peer_id, addr);
                    self.kademlia.add_address(&peer_id, addr);
                }
            }
        }

        Ok(())
    }

    async fn handle_request_response_event(
        &mut self,
        request_event: request_response::Event<Request, Response>,
        my_open_mls: &MyOpenMls,
        pool: &sqlx::PgPool,
    ) -> Result<(), AbstSwarmError> {
        log::debug!("Received Request::Event: {:?}", request_event);
        if let request_response::Event::Message { message, peer, .. } = request_event {
            match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    // A joining peer has no group key yet, so it asks unsigned. The key is public;
                    // answering over this connection authenticates it as this archive's key.
                    if let RequestMessage::RequestOpenerKeys = request.body {
                        self.request_response
                            .send_response(
                                channel,
                                Response {
                                    body: ResponseMessage::ArchiveOpenerKey {
                                        elgamal_pk: my_open_mls.get_elgamal_pubkey()?,
                                    },
                                    signature: None,
                                },
                            )
                            .map_err(|_| SendResponseError)?;
                        return Ok(());
                    }
                    //Verify Group Signature
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
                    //Return Update
                    match request_body {
                        RequestMessage::Update { group_id, epoch } => {
                            let mut messages = Vec::new();
                            messages.append(
                                &mut Self::get_newer_commits(pool, &group_id, epoch).await?,
                            );
                            messages.append(
                                &mut Self::get_newer_proposals(pool, &group_id, epoch).await?,
                            );

                            self.request_response
                                .send_response(
                                    channel,
                                    Response {
                                        body: ResponseMessage::ResUpdate { group_id, messages },
                                        signature: None,
                                    },
                                )
                                .map_err(|_| SendResponseError)?;
                        }

                        RequestMessage::AddIDShare {
                            id,
                            enc_share,
                            commits,
                        } => {
                            println!("Got a new ID Share");
                            //Verify Share and Add to DB
                            if my_open_mls.verify_enc_share(&enc_share, &commits, None) {
                                let commits = bincode::serialize(&commits).unwrap();
                                let enc_share = enc_share.to_bytes();
                                Self::insert_id_enc_share(pool, id, &enc_share, &commits).await?;
                            }
                            //Add to DB
                            self.request_response
                                .send_response(
                                    channel,
                                    Response {
                                        body: ResponseMessage::AddPeerShare,
                                        signature: None,
                                    },
                                )
                                .map_err(|_| SendResponseError)?;
                        }

                        RequestMessage::AddPeerShare {
                            peer_id,
                            enc_share,
                            commits,
                        } => {
                            println!("Got a new Peer Share");
                            //Verify Share and Add to DB
                            if my_open_mls.verify_enc_share(&enc_share, &commits, None) {
                                let commits = bincode::serialize(&commits).unwrap();
                                let enc_share = enc_share.to_bytes();
                                let peer_id = peer_id.to_bytes();
                                Self::insert_peer_enc_share(pool, &peer_id, &enc_share, &commits)
                                    .await?;
                            }
                            //Add to DB
                            self.request_response
                                .send_response(
                                    channel,
                                    Response {
                                        body: ResponseMessage::AddPeerShare,
                                        signature: None,
                                    },
                                )
                                .map_err(|_| SendResponseError)?;
                        }

                        RequestMessage::JoinGroup {
                            welcome,
                            ratchet_tree,
                            ..
                        } => {
                            //Only add to commongroup if it comes from authority
                            if my_open_mls.get_authorityid() == peer {
                                log::info!(
                                    "Received a JoinGroup Request from Authority {}",
                                    peer.to_string()
                                );
                                let group_id =
                                    my_open_mls.add_group_from_welcome(welcome, ratchet_tree)?;
                                my_open_mls.move_to_common_id(group_id.clone())?;
                                log::info!("Added Common Group {group_id:?}");
                                self.subscribe(group_id.clone(), my_open_mls)?;
                                self.request_response
                                    .send_response(
                                        channel,
                                        Response {
                                            body: ResponseMessage::JoinGroup,
                                            signature: None,
                                        },
                                    )
                                    .map_err(|_| SendJoinGroupError { group_id })?
                            }
                        }

                        // The archive decides on every opening itself; the authority can only ask.
                        RequestMessage::RequestOpen { signature } => {
                            if my_open_mls.get_authorityid() == peer {
                                let id = open_request_id(&signature);
                                if *AUTO_APPROVE_OPEN.get().unwrap_or(&false) {
                                    log::warn!(
                                        "Auto-approving opening request {id} (--auto-approve-open)"
                                    );
                                    self.submit_open_pairings(pool, my_open_mls, signature)
                                        .await?;
                                } else {
                                    Self::queue_open(pool, &id, &signature).await?;
                                    println!(
                                        "Opening request {id} from the authority is pending. Check the evidence, then run `approve {id}` or `deny {id}`."
                                    );
                                }
                                self.request_response
                                    .send_response(
                                        channel,
                                        Response {
                                            body: ResponseMessage::OpenPending,
                                            signature: None,
                                        },
                                    )
                                    .map_err(|_| SendResponseError)?;
                            }
                        }

                        _ => {}
                    }
                }

                request_response::Message::Response { response, .. } => {
                    match response.body {
                        ResponseMessage::IdentityVerified { group_key, .. } => {
                            log::info!(
                                "Identity was verified, Got GroupKey, Sending out KeyPackage"
                            );
                            let keypackage = my_open_mls.generate_key_package()?;
                            let keypackage = bincode::serialize(&keypackage)
                                .map_err(|x| SerializationError(x.to_string()))?;
                            my_open_mls.set_grpkey(group_key)?;
                            my_open_mls.generate_elgamal_key_pair()?;
                            self.request_response.send_request(
                                &peer,
                                Request {
                                    body: RequestMessage::JoinKeyPackage {
                                        keypackage,
                                        elgamal_pk: my_open_mls.get_elgamal_pubkey()?,
                                    },
                                    signature: None,
                                },
                            );
                        }
                        ResponseMessage::MemKey { .. } => {}
                        ResponseMessage::OpenPairingsReceived => {
                            log::info!("Authority received our partial opening");
                        }

                        //Ignore all other Responses for now
                        _ => {}
                    }
                }
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
            } => match peer_id {
                None => log::warn!("Outgoing connection error (no peer id): {}", error),
                Some(peer_id) => log::error!("Could not connect to peer '{}': {}", peer_id, error),
            },
            SwarmEvent::ConnectionClosed {
                peer_id,
                cause,
                ..
            } => {
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
impl EventHandler for MyBehaviour {
    async fn handle_event(
        &mut self,
        event: SwarmEvent<Self::ToSwarm>,
        pool: &sqlx::PgPool,
        my_open_mls: &MyOpenMls,
    ) {
        let connected = match &event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => Some(*peer_id),
            _ => None,
        };
        if let Err(e) = match event {
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(event)) => {
                self.handle_pubsub_event(*event, pool, my_open_mls).await
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::IdentifyEvent(e)) => {
                self.handle_identify_event(e)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::RequestResponseEvent(e)) => {
                self.handle_request_response_event(*e, my_open_mls, pool)
                    .await
            }
            non_functional_event => self.handle_non_functional_event(non_functional_event),
        } {
            log::error!("{e:?}")
        }
        admission::update(&mut self.gossipsub, my_open_mls, connected.as_ref());
    }
}

#[async_trait]
impl InstructionHandler for MyBehaviour {
    async fn handle_instruction(
        &mut self,
        instruction: Instruction,
        pool: &sqlx::PgPool,
        my_open_mls: &MyOpenMls,
    ) {
        let result = match instruction {
            Instruction::RegisterIdentity { peer_id, identity } => {
                self.register_identity(&peer_id, identity);
                Ok(())
            }
            Instruction::ListPendingOpens => self.print_pending_opens(pool).await,
            Instruction::ApproveOpen { id } => self.approve_open(pool, my_open_mls, &id).await,
            Instruction::DenyOpen { id } => self.deny_open(pool, &id).await,
            _ => Ok(()),
        };
        if let Err(e) = result {
            log::error!("{e:?}")
        }
    }
}

#[derive(Debug)]
pub(crate) enum MyBehaviourEvent {
    Gossipsub(Box<gossipsub::Event>),
    KademliaEvent(()),
    IdentifyEvent(Box<identify::Event>),
    RequestResponseEvent(Box<request_response::Event<Request, Response>>),
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
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}
