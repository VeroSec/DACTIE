use crate::abstswarm::Instruction;
use crate::db::Db;
use dactie_utils::mls_wrapper::MyOpenMls;
use libp2p::swarm::NetworkBehaviour;

/// Handles operator commands on the event loop; like `EventHandler`, without waiting.
pub(crate) trait InstructionHandler
where
    Self: NetworkBehaviour,
{
    fn handle_instruction(&mut self, instruction: Instruction, db: &mut Db, my_open_mls: &MyOpenMls);
}
