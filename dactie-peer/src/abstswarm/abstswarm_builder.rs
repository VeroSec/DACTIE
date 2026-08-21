use std::convert::Infallible;
use std::io;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::mpsc;

use crate::abstswarm::event_handler::EventHandler;
use crate::abstswarm::instruction_handler::InstructionHandler;
use crate::abstswarm::{AbstSwarm, Instruction, Notification};
use dactie_utils::mls_wrapper::MyOpenMls;
use identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{identity, multiaddr, noise, tcp, yamux, Multiaddr, TransportError};
use libp2p_metrics as lp2p_metrics;
use openmls::group::GroupId;

pub(crate) struct AbstSwarmBuilder<TBehaviour> {
    keypair: Keypair,
    instruction_rx: mpsc::UnboundedReceiver<Instruction>,
    notification_tx: mpsc::UnboundedSender<Notification>,
    listen_address: Multiaddr,
    behaviour: TBehaviour,
    my_open_mls: MyOpenMls,
    group_commit_rx: mpsc::UnboundedReceiver<GroupId>,
    metrics: Option<lp2p_metrics::Metrics>,
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
        notification_tx: mpsc::UnboundedSender<Notification>,
        behaviour: TBehaviour,
        my_open_mls: MyOpenMls,
        group_commit_rx: mpsc::UnboundedReceiver<GroupId>,
    ) -> Result<AbstSwarmBuilder<TBehaviour>, AbstSwarmBuilderError> {
        Ok(AbstSwarmBuilder {
            listen_address: Self::default_listen_address()?,
            keypair,
            instruction_rx,
            notification_tx,
            behaviour,
            my_open_mls,
            group_commit_rx,
            metrics: None,
            external_addresses: Vec::new(),
        })
    }

    pub(crate) fn listen_address(mut self, address: Multiaddr) -> Self {
        self.listen_address = address;
        self
    }

    /// Listen on all interfaces, on a random port
    fn default_listen_address() -> Result<Multiaddr, AbstSwarmBuilderError> {
        Ok("/ip4/0.0.0.0/tcp/0".parse()?)
    }

    pub(crate) fn with_metrics(mut self, metrics: lp2p_metrics::Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Add a host-reachable address to advertise via Identify/Kademlia.
    pub(crate) fn add_external_address(mut self, address: Multiaddr) -> Self {
        self.external_addresses.push(address);
        self
    }

    pub(crate) fn build(self) -> Result<AbstSwarm<TBehaviour>, AbstSwarmBuilderError> {
        // Build swarm as before
        let mut swarm = {
            libp2p::SwarmBuilder::with_existing_identity(self.keypair)
                .with_tokio()
                .with_tcp(
                    tcp::Config::default(),
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
            self.instruction_rx,
            self.notification_tx,
            self.group_commit_rx,
            swarm,
            self.my_open_mls,
            self.metrics,
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
