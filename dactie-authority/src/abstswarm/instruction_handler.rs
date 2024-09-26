use async_trait::async_trait;
use libp2p::swarm::NetworkBehaviour;
use tokio::sync::mpsc;
use crate::abstswarm::{Instruction, Notification};
use dactie_utils::mls_wrapper::MyOpenMls;
use crate::storage::Storage;

#[async_trait]
pub(crate) trait InstructionHandler
where
    Self: NetworkBehaviour,
{
    async fn handle_instruction(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        instruction: Instruction,
        storage: &Storage
    );
}