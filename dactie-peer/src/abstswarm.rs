pub(crate) mod abstswarm_builder;
pub(crate) mod event_handler;
pub(crate) mod instruction_handler;

use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

use dactie_utils::mls_wrapper::{MyOpenMLSError, MyOpenMls};
use dactie_utils::shared_structs::{Instruction, MessageBody};
use event_handler::EventHandler;
use instruction_handler::InstructionHandler;
use libp2p::futures::StreamExt;
use libp2p::swarm::NetworkBehaviour;
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
}

impl<TBehaviour> AbstSwarm<TBehaviour>
where
    TBehaviour: NetworkBehaviour + EventHandler + InstructionHandler,
{
    pub(crate) fn new(
        instruction_rx: mpsc::UnboundedReceiver<Instruction>,
        notification_tx: mpsc::UnboundedSender<Notification>,
        group_commit_rx: mpsc::UnboundedReceiver<GroupId>,
        swarm: Swarm<TBehaviour>,
        my_open_mls: Arc<MyOpenMls>,
        metrics: Option<lp2p_metrics::Metrics>,
    ) -> Self {
        AbstSwarm {
            instruction_rx,
            notification_tx,
            swarm,
            my_open_mls,
            group_commit_rx,
            metrics,
            join_relay: Default::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        loop {
            tokio::select! {
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
