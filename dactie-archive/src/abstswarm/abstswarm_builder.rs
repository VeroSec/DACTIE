use std::convert::Infallible;
use std::io;
use std::time::Duration;

use thiserror::Error;

use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::instruction_handler::InstructionHandler;
use crate::abstswarm::AbstSwarm;
use dactie_utils::mls_wrapper::MyOpenMls;
use dactie_utils::shared_structs::Instruction;
use identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{identity, multiaddr, noise, tcp, yamux, Multiaddr, TransportError};
use sqlx::{Pool, Postgres};
use tokio::sync::mpsc;

pub(crate) struct AbstSwarmBuilder<TBehaviour> {
    keypair: Keypair,
    instruction_rx: mpsc::UnboundedReceiver<Instruction>,
    listen_address: Multiaddr,
    pool: Pool<Postgres>,
    behaviour: TBehaviour,
    my_open_mls: MyOpenMls,
    // Addresses to advertise to remote peers as dialable
    external_addresses: Vec<Multiaddr>,

}

impl<TBehaviour> AbstSwarmBuilder<TBehaviour>
where
    TBehaviour: NetworkBehaviour + EventHandler + InstructionHandler,
{
    pub(crate) fn new(
        keypair: Keypair,
        instruction_rx: mpsc::UnboundedReceiver<Instruction>,
        pool: Pool<Postgres>,
        behaviour: TBehaviour,

        my_open_mls: MyOpenMls,
    ) -> Result<AbstSwarmBuilder<TBehaviour>, AbstSwarmBuilderError> {
        Ok(AbstSwarmBuilder {
            listen_address: Self::default_listen_address()?,
            instruction_rx,
            keypair,
            pool,
            behaviour,
            my_open_mls,
            external_addresses: Vec::new(),
        })
    }

    pub(crate) fn listen_address(mut self, address: Multiaddr) -> Self {
        self.listen_address = address;
        self
    }

    /// Add a host-reachable address to advertise via Identify/Kademlia.
    pub(crate) fn add_external_address(mut self, address: Multiaddr) -> Self {
        self.external_addresses.push(address);
        self
    }


    /// Listen on all interfaces, on a random port
    fn default_listen_address() -> Result<Multiaddr, AbstSwarmBuilderError> {
        Ok("/ip4/0.0.0.0/tcp/0".parse()?)
    }

    pub(crate) fn build(self) -> Result<AbstSwarm<TBehaviour>, AbstSwarmBuilderError> {
        let mut swarm = {
            libp2p::SwarmBuilder::with_existing_identity(self.keypair)
                .with_tokio()
                .with_tcp(
                    // Nagle off: libp2p's default sets TCP_NODELAY to false, which
                    // holds small writes back for up to one round trip per hop.
                    tcp::Config::default().nodelay(true),
                    noise::Config::new,
                    yamux::Config::default,
                )?
                .with_behaviour(|_| self.behaviour)?
                .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(5)))
                .build()
        };
        swarm.listen_on(self.listen_address)?;
        // Advertise explicit external addresses so other peers dial the correct host:port
        for addr in &self.external_addresses {
            log::info!("Advertising external address: {}", addr);
            libp2p::Swarm::add_external_address(&mut swarm, addr.clone());
        }

        log::info!("Local PeerId: {}", swarm.local_peer_id());

        Ok(AbstSwarm::new(
            self.pool,
            self.instruction_rx,
            swarm,
            self.my_open_mls,
        ))
    }
}

#[derive(Debug, Error)]
pub(crate) enum AbstSwarmBuilderError {
    #[error(transparent)]
    BuildError(#[from] TransportError<io::Error>),
    #[error(transparent)]
    ListenAddressError(#[from] multiaddr::Error),
    #[error(transparent)]
    TransportError(#[from] noise::Error),
    #[error(transparent)]
    BuildError2(#[from] Infallible),
}
