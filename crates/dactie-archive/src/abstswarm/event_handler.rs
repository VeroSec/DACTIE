use crate::db::{Db, Deferred};
use dactie_utils::mls_wrapper::MyOpenMls;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};

/// Handles swarm events on the event loop. Nothing here may wait: while the loop waits, it
/// reads no connection and every neighbour buffers for this archive. Database work goes
/// through `Db` (see crate::db).
pub(crate) trait EventHandler
where
    Self: NetworkBehaviour,
{
    fn handle_event(
        &mut self,
        event: SwarmEvent<Self::ToSwarm>,
        db: &mut Db,
        my_open_mls: &MyOpenMls,
    );

    /// Finishes database work that ran in its own task.
    fn handle_deferred(&mut self, deferred: Deferred, my_open_mls: &MyOpenMls);
}
