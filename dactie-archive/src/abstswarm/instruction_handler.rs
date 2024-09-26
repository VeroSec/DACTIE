use async_trait::async_trait;
use libp2p::swarm::NetworkBehaviour;
use crate::abstswarm::{Instruction};


#[async_trait]
pub(crate) trait InstructionHandler
where
    Self: NetworkBehaviour,
{
    async fn handle_instruction(
        &mut self,
        instruction: Instruction,
    );
}