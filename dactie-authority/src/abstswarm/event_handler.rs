use crate::abstswarm::Notification;
use crate::storage::Storage;
use async_trait::async_trait;
use dactie_utils::mls_wrapper::MyOpenMls;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use tokio::sync::mpsc;

#[async_trait]
pub(crate) trait EventHandler
where
    Self: NetworkBehaviour,
{
    async fn handle_event(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        storage: &Storage,
        event: SwarmEvent<Self::ToSwarm>,
    );

    /// Called periodically: removes sharing peers whose registration has expired.
    fn expire_peers(&mut self, my_open_mls: &MyOpenMls, storage: &Storage);
}
