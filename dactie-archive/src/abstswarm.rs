pub(crate) mod event_handler;
pub(crate) mod abstswarm_builder;
pub(crate) mod instruction_handler;

use std::fmt::Debug;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use libp2p::futures::StreamExt;
pub(crate) use libp2p::PeerId;
use libp2p::Swarm;
use libp2p::swarm::NetworkBehaviour;
use sqlx::{Pool, Postgres};
use tokio::sync::mpsc;
use dactie_utils::mls_wrapper::{MyOpenMls, MyOpenMLSError};
use event_handler::EventHandler;
use crate::mybehaviour::MyBehaviourError;
use dactie_utils::shared_structs::{Instruction};
use crate::abstswarm::instruction_handler::InstructionHandler;

pub(crate) struct AbstSwarm<TBehaviour>
where
    TBehaviour: NetworkBehaviour,
{
    pool: Pool<Postgres>,
    instruction_rx: mpsc::UnboundedReceiver<Instruction>,
    swarm: Swarm<TBehaviour>,
    my_open_mls: MyOpenMls
}

impl<TBehaviour> AbstSwarm<TBehaviour>
where
    TBehaviour: NetworkBehaviour + EventHandler + InstructionHandler,
{
    pub(crate) fn new(
        pool: Pool<Postgres>,
        instruction_rx: mpsc::UnboundedReceiver<Instruction>,
        swarm: Swarm<TBehaviour>,
        my_open_mls: MyOpenMls,
    ) -> Self {
        AbstSwarm { pool, instruction_rx ,swarm, my_open_mls}
    }

    pub(crate) async fn run(mut self) {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => self.swarm.behaviour_mut().handle_event(event, &self.pool, &self.my_open_mls).await,
                Some(instruction) = self.instruction_rx.recv() =>  self.swarm.behaviour_mut().handle_instruction(instruction).await,
                else => {
                    log::warn!("Both swarm and instruction receiver closed. Ending event loop");
                    break

                }
            }
        }
    }
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
    #[error("Behaviour Error")]
    BehaviourError(#[from] MyBehaviourError),
    #[error("MLS Error: {0}")]
    MLSError(#[from] MyOpenMLSError),
    #[error("Failed to validate Message")]
    MessageValidationError,
    #[error("Failed to send Joingrequest to group `{group_id:?}`")]
    SendJoinGroupError { group_id: Vec<u8>},
    #[error("Failed to send response")]
    SendResponseError,
}
