use crate::abstswarm::Notification;
use async_trait::async_trait;
use dactie_utils::mls_wrapper::MyOpenMls;
use dactie_utils::shared_structs::Response;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::PeerId;
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
        event: SwarmEvent<Self::ToSwarm>,
    );

    /// A response to a request the join relay sent from its throwaway identity.
    async fn handle_relayed_response(
        &mut self,
        notification_tx: &mpsc::UnboundedSender<Notification>,
        my_open_mls: &MyOpenMls,
        peer: PeerId,
        response: Response,
    );
}
