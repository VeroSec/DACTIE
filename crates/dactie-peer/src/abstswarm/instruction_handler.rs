use crate::abstswarm::{Instruction, Notification};
use async_trait::async_trait;
use dactie_utils::mls_wrapper::MyOpenMls;
use libp2p::swarm::NetworkBehaviour;
use tokio::sync::mpsc;

// async_trait marks the boxed future #[must_use] a second time.
#[allow(clippy::double_must_use)]
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
    );
}
