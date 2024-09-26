pub(crate) mod event_handler;
pub(crate) mod instruction_handler;
pub(crate) mod abstswarm_builder;

use std::fmt::Debug;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

use libp2p::futures::StreamExt;
use libp2p::PeerId;
use libp2p::Swarm;
use libp2p::swarm::NetworkBehaviour;
use event_handler::EventHandler;
use instruction_handler::InstructionHandler;
use openmls::group::GroupId;
use dactie_utils::shared_structs::{MessageBody,Instruction};
use dactie_utils::mls_wrapper::{MyOpenMls, MyOpenMLSError};

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
    my_open_mls: Arc<MyOpenMls>
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
        my_open_mls: MyOpenMls
    ) -> Self {
        AbstSwarm { instruction_rx, notification_tx, swarm, my_open_mls: Arc::new(my_open_mls), group_commit_rx}
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
                event = self.swarm.select_next_some() => self.swarm.behaviour_mut().handle_event(&self.notification_tx,&self.my_open_mls, event).await,
                Some(instruction) = self.instruction_rx.recv() =>  self.swarm.behaviour_mut().handle_instruction(&self.notification_tx, &self.my_open_mls,instruction).await,
                else => {
                    log::warn!("Both swarm and instruction receiver closed. Ending event loop");
                    break

                }
            }
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
        signature: Vec<u8>
    },
    Err(AbstSwarmError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Message<T: Debug + Clone> {
    pub(crate) source: PeerId,
    pub(crate) body: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Address {
    Broadcast(String),
    Group(usize)
}

#[derive(Debug, Error)]
pub(crate) enum AbstSwarmError {
    #[error("Failed to send broadcast: `{reason:?}`")]
    SendError { reason: String },
    #[error("Failed to send Joingrequest to group `{group_id:?}`")]
    SendJoinGroupError { group_id: Vec<u8>},
    #[error("Failed to handle update: `{reason:?}`")]
    HandleUpdateError{ reason: String },
    #[error("Failed to send keypackage response")]
    SendKeyPackageError,
    #[error("Failed to subscribe to topic: `{reason:?}`")]
    SubscriptionError { reason: String },
    #[error("MLS Error: {0}")]
    MLSError(#[from] MyOpenMLSError),
    #[error("Failed to validate Message")]
    MessageValidationError,

}