pub(crate) mod abstswarm_builder;
pub(crate) mod event_handler;
pub(crate) mod instruction_handler;

use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::mpsc;

use dactie_utils::mls_wrapper::{MyOpenMLSError, MyOpenMls};
use dactie_utils::shared_structs::{Instruction, MessageBody};
use event_handler::EventHandler;
use instruction_handler::InstructionHandler;
use libp2p::futures::StreamExt;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::PeerId;
use libp2p::Swarm;
use libp2p_metrics as lp2p_metrics;
use openmls::group::GroupId;

///
/// The `AbstSwarm` is a convenience wrapper around the `Swarm` for a `NetworkBehaviour`.
/// It can be constructed with the `AbstSwarmBuilder` for convenience.
///
/// Communication between the `Network` and its client(s) happens only through mpsc channels,
/// and is fully async.
///
///
/// The `AbstSwarm`:
/// - receives `Instructions`, which it passes on to the `InstructionHandler` trait implemented
///   on the swarm's NetworkBehaviour.
/// - receives events from the swarm and passes them on to the `EventHandler` trait implemented
///   on the swarm's `NetworkBehaviour`. The `EventHandler` can notify the `Network`'s client
///   by sending a `Notification`.
///
/// The `Network` does not know or care what the Instructions and Notifications contain. It is up to
/// the client(s) to give them meaning.
///
/// Peers a behaviour must stay connected to when connections are trimmed.
pub(crate) trait ProtectedPeers {
    fn protected_peers(&self) -> HashSet<PeerId>;
}

/// New connections are not trimmed during their first seconds (go-libp2p's
/// connection manager uses the same 20 s grace period).
const CONN_GRACE: Duration = Duration::from_secs(20);

pub(crate) struct AbstSwarm<TBehaviour>
where
    TBehaviour: NetworkBehaviour,
{
    instruction_rx: mpsc::UnboundedReceiver<Instruction>,
    notification_tx: mpsc::UnboundedSender<Notification>,
    group_commit_rx: mpsc::UnboundedReceiver<GroupId>,
    swarm: Swarm<TBehaviour>,
    my_open_mls: Arc<MyOpenMls>,
    metrics: Option<lp2p_metrics::Metrics>,
    join_relay: crate::join_relay::JoinRelay,
    /// Connection watermarks: once a peer holds `conn_high` connections, it closes
    /// unprotected ones down to `conn_low` so that newcomers find free slots.
    conn_low: usize,
    conn_high: usize,
    conn_since: HashMap<PeerId, Instant>,
}

impl<TBehaviour> AbstSwarm<TBehaviour>
where
    TBehaviour: NetworkBehaviour + EventHandler + InstructionHandler + ProtectedPeers,
{
    pub(crate) fn new(
        instruction_rx: mpsc::UnboundedReceiver<Instruction>,
        notification_tx: mpsc::UnboundedSender<Notification>,
        group_commit_rx: mpsc::UnboundedReceiver<GroupId>,
        swarm: Swarm<TBehaviour>,
        my_open_mls: Arc<MyOpenMls>,
        metrics: Option<lp2p_metrics::Metrics>,
        (conn_low, conn_high): (usize, usize),
    ) -> Self {
        AbstSwarm {
            instruction_rx,
            notification_tx,
            swarm,
            my_open_mls,
            group_commit_rx,
            metrics,
            join_relay: Default::default(),
            conn_low,
            conn_high,
            conn_since: HashMap::new(),
        }
    }

    /// Once the peer is full, close connections down to the low watermark. The
    /// authority, the behaviour's protected peers (gossipsub mesh, trusted
    /// archives) and connections younger than the grace period are kept.
    fn trim_connections(&mut self) {
        let connected: Vec<PeerId> = self.swarm.connected_peers().copied().collect();
        if connected.len() < self.conn_high {
            return;
        }
        let mut protected = self.swarm.behaviour().protected_peers();
        protected.insert(self.my_open_mls.get_authorityid());
        let now = Instant::now();
        let mut candidates: Vec<PeerId> = connected
            .iter()
            .filter(|p| !protected.contains(*p))
            .filter(|p| {
                self.conn_since
                    .get(*p)
                    .is_none_or(|since| now.duration_since(*since) >= CONN_GRACE)
            })
            .copied()
            .collect();
        candidates.shuffle(&mut rand::thread_rng());
        let excess = connected.len() - self.conn_low;
        for peer in candidates.into_iter().take(excess) {
            let _ = self.swarm.disconnect_peer_id(peer);
        }
    }

    pub(crate) async fn run(mut self) {
        let mut trim = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = trim.tick() => self.trim_connections(),
                Some(group_id) = self.group_commit_rx.recv() => {
                    let my_open_mls_clone = self.my_open_mls.clone();
                    let notification_tx_clone= self.notification_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = my_open_mls_clone.commit(group_id).await{
                            if let Err(e) = notification_tx_clone.send(Notification::Err(e.into())) {
                            log::error!("Failed to send notification back to router through mpsc channel: {e}");
                            }
                        }
                    });
                },
                event = self.swarm.select_next_some() => {
                    // Record swarm-level event metrics (counts, protocol stats)
                    if let Some(metrics) = &self.metrics {
                        lp2p_metrics::Recorder::record(metrics, &event);
                    }
                    match &event {
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            self.conn_since.entry(*peer_id).or_insert_with(Instant::now);
                        }
                        SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                            self.conn_since.remove(peer_id);
                        }
                        _ => {}
                    }
                    self.swarm.behaviour_mut().handle_event(&self.notification_tx,&self.my_open_mls, event).await
                },
                Some(instruction) = self.instruction_rx.recv() =>  self.swarm.behaviour_mut().handle_instruction(&self.notification_tx, &self.my_open_mls,instruction).await,
                (peer, response) = self.join_relay.next_response() => self.swarm.behaviour_mut().handle_relayed_response(&self.notification_tx, &self.my_open_mls, peer, response).await,
                else => {
                    log::warn!("Both swarm and instruction receiver closed. Ending event loop");
                    break

                }
            }
            // Handlers queue the messages of a join for the throwaway identity.
            self.join_relay.flush();
        }
    }
}

/// A notification from the swarm to one of its consumers.
/// Either arbitrary data, the list of known peers or an error.
#[derive(Debug)]
pub(crate) enum Notification {
    Data {
        propagation_source: PeerId,
        message_body: MessageBody,
        signature: Vec<u8>,
        // MemberID of an attributed message, verified against its signature
        attributed_to: Option<usize>,
    },
    Err(AbstSwarmError),
}


#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum AbstSwarmError {
    #[error("Failed to send broadcast: `{reason:?}`")]
    SendError { reason: String },
    #[error("Failed to send Joingrequest to group `{group_id:?}`")]
    SendJoinGroupError { group_id: Vec<u8> },
    #[error("Failed to handle update: `{reason:?}`")]
    HandleUpdateError { reason: String },
    #[error("Failed to send keypackage response")]
    SendKeyPackageError,
    #[error("Failed to subscribe to topic: `{reason:?}`")]
    SubscriptionError { reason: String },
    #[error("MLS Error: {0}")]
    MLSError(#[from] MyOpenMLSError),
    #[error("Failed to validate Message")]
    MessageValidationError,
    #[error("All slots of epoch {epoch} are used by this member (see {dir}/slots.json)")]
    NoFreeSlot { dir: String, epoch: u64 },
    #[error("Could not record the used slot: {reason}")]
    SlotStoreError { reason: String },
    #[error("Epoch length and slots are unknown; run get_opener_keys first")]
    NoEpochPolicy,
}
