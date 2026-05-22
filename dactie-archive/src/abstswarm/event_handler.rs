use async_trait::async_trait;
use libp2p::swarm::{ NetworkBehaviour, SwarmEvent};
use dactie_utils::mls_wrapper::MyOpenMls;

#[async_trait]
pub(crate) trait EventHandler
where
    Self: NetworkBehaviour,
{
    async fn handle_event(
        &mut self,
        event: SwarmEvent<Self::ToSwarm>,
        pool: &sqlx::PgPool,
        my_open_mls: &MyOpenMls
    );
}