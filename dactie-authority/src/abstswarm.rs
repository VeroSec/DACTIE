pub(crate) mod abstswarm_builder;
pub(crate) mod event_handler;
pub(crate) mod instruction_handler;

use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::storage::{MyStorageError, Storage};
use dactie_utils::mls_wrapper::{MyOpenMLSError, MyOpenMls};
use dactie_utils::shared_structs::Instruction;
use event_handler::EventHandler;
use instruction_handler::InstructionHandler;
use libp2p::futures::StreamExt;
use libp2p::swarm::NetworkBehaviour;
use libp2p::Swarm;
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
    storage: Storage,
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
        my_open_mls: MyOpenMls,
        storage: Storage,
    ) -> Self {
        AbstSwarm {
            instruction_rx,
            notification_tx,
            swarm,
            my_open_mls: Arc::new(my_open_mls),
            storage,
            group_commit_rx,
        }
    }

    pub(crate) async fn run(mut self) {
        // Sharing peers expire at the end of their epoch, without any event to trigger it.
        let mut expiry = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = expiry.tick() => self.swarm.behaviour_mut().expire_peers(&self.my_open_mls, &self.storage),
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
                event = self.swarm.select_next_some() => self.swarm.behaviour_mut().handle_event(&self.notification_tx,&self.my_open_mls, &self.storage ,event).await,
                Some(instruction) = self.instruction_rx.recv() =>  self.swarm.behaviour_mut().handle_instruction(&self.notification_tx, &self.my_open_mls,instruction, &self.storage).await,
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
    Err(AbstSwarmError),
}


#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum AbstSwarmError {
    #[error("Failed to send broadcast: `{reason:?}`")]
    SendError { reason: String },
    #[error("MLS Error: {0}")]
    MLSError(#[from] MyOpenMLSError),
    #[error("MLS Error: {0}")]
    StorageError(#[from] Box<MyStorageError>),
}

impl From<MyStorageError> for AbstSwarmError {
    fn from(err: MyStorageError) -> Self {
        AbstSwarmError::StorageError(Box::new(err))
    }
}
