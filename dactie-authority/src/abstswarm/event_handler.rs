use async_trait::async_trait;
use libp2p::swarm::{ NetworkBehaviour, SwarmEvent};
use tokio::sync::mpsc;
use crate::abstswarm::Notification;
use dactie_utils::mls_wrapper::MyOpenMls;
use crate::storage::Storage;

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
}