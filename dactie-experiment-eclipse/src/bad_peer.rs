use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use futures::StreamExt;
use libp2p::{gossipsub, identify, kad, noise, tcp, yamux, Multiaddr, PeerId, Swarm};
use libp2p::gossipsub::{IdentTopic, MessageAcceptance};
use libp2p::kad::{Mode, PROTOCOL_NAME};
use libp2p::kad::store::MemoryStore;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use tokio::{io, select};
use tokio::sync::broadcast;
use tokio::sync::mpsc::{UnboundedSender};
use tokio::time::{self, Duration, Interval};
use rand::{distributions::Alphanumeric, Rng};
use crate::shared_structs::*;

pub(crate) struct Peer {
    swarm: Swarm<MyBehaviour>,
    response_channel: UnboundedSender<PeerOut>,
    control_channel: broadcast::Receiver<ControlSignal>,
    censor_peer: PeerId,
    attack_mode: bool,


}


#[derive(NetworkBehaviour)]
pub(crate) struct MyBehaviour {
    /// The Gossipsub pub/sub behaviour is used to send broadcast messages to peers.
    pub(crate) gossipsub: gossipsub::Behaviour,
    /// Send more detailed identifying info to connected peers, a.o the listen_address.
    /// This address can then be used to populate the Kademlia DHT.
    pub(crate) identify: identify::Behaviour,
    /// The Kademlia DHT used to discover peers
    pub(crate) kademlia: kad::Behaviour<MemoryStore>,
}

impl Peer {

    pub(crate) fn new_peer(boot_peer_id: PeerId,control_channel: broadcast::Receiver<ControlSignal>, response_channel: UnboundedSender<PeerOut>, censor_peer: PeerId) -> Result<Peer,Box<dyn Error>>{
        let swarm = Self::create_swarm(Some((boot_peer_id,"/ip4/127.0.0.1/tcp/10001".parse()? )))?;

        Ok(Peer{swarm, control_channel, response_channel, censor_peer, attack_mode:false})
    }

    fn create_swarm(bootstrap_peer: Option<(PeerId, Multiaddr)>) -> Result<Swarm<MyBehaviour>, Box<dyn Error>> {
        let mut swarm = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                // To content-address message, we can take the hash of message and use it as an ID.
                let message_id_fn = |message: &gossipsub::Message| {
                    let mut s = DefaultHasher::new();
                    message.data.hash(&mut s);
                    gossipsub::MessageId::from(s.finish().to_string())
                };

                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(10)) // This is set to aid debugging by not cluttering the log space
                    .validation_mode(gossipsub::ValidationMode::Permissive) // This sets the kind of message validation. The default is Strict (enforce message signing)
                    .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
                    .flood_publish(false)
                    .validate_messages()
                    .build()
                    .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg)).unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.

                let mut gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Anonymous,
                    gossipsub_config,
                )?;

                gossipsub
                    .subscribe(&IdentTopic::new("Test"))?;

                let identify = identify::Behaviour::new(identify::Config::new(
                    identify::PROTOCOL_NAME.to_string(),
                    key.public(),
                ));

                let peer_id = key.public().to_peer_id();

                let mut cfg = kad::Config::new(PROTOCOL_NAME);
                cfg.set_query_timeout(Duration::from_secs(5 * 60));
                let store = MemoryStore::new(peer_id);
                let mut kademlia = kad::Behaviour::with_config(peer_id, store, cfg);

                kademlia.set_mode(Some(Mode::Server));

                if let Some((boot_peer_id, boot_peer_addr)) = bootstrap_peer.clone() {
                    // Add the bootnodes to the local routing table.
                    kademlia.add_address(&boot_peer_id, boot_peer_addr);
                }

                Ok(MyBehaviour {
                    gossipsub,
                    identify,
                    kademlia,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();


        swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

        if None == bootstrap_peer {
            swarm.listen_on("/ip4/0.0.0.0/tcp/10001".parse()?)?;
        } else {
            swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        }


        Ok(swarm)
    }

    pub(crate) fn get_peer_id(&mut self) -> PeerId{
        self.swarm.local_peer_id().clone()
    }

    pub(crate) async fn run(mut self, pub_interval: PubInterval){

        let mut timer: Option<Interval> = None;
        loop {
            select! {
                Ok(signal) = self.control_channel.recv() => {
                    match signal {
                        ControlSignal::Run => {
                            if timer.is_none() {
                                let secs = match pub_interval{
                                    PubInterval::Fix(val) => val,
                                    PubInterval::Random((min,max)) => rand::thread_rng().gen_range(min..=max).try_into().unwrap()
                                };
                                timer = Some(time::interval(Duration::from_secs(secs)));
                            }
                        }
                        ControlSignal::Stop => {
                            break
                        },
                        ControlSignal::Attack => {
                            self.attack_mode = true;
                        }
                        ControlSignal::Pub(_) => {}
                    }

                },
                _ = Self::tick(&mut timer), if timer.is_some() => {

                    let message = generate_random_string(120);
                    let _ = self.swarm.behaviour_mut().gossipsub.publish(IdentTopic::new("Test"),message); //Not handled
                },
                event = self.swarm.select_next_some() => self.handle_event(event)
            }
        }
    }

    fn handle_event(&mut self, event: SwarmEvent<MyBehaviourEvent>){

        match event {
            SwarmEvent::Behaviour(MyBehaviourEvent::Identify(e)) => {
                log::debug!("Received identify::Event: {:?}", e);

                if let identify::Event::Received {
                    peer_id,
                    info:
                    identify::Info {
                        listen_addrs,
                        protocols,
                        ..
                    },
                } = e
                {
                    if protocols
                        .iter()
                        .any(|p| *p == PROTOCOL_NAME)
                    {
                        for addr in listen_addrs {
                            log::debug!("Adding received IdentifyInfo matching protocol  to the DHT. Peer: {}, addr: {}", peer_id, addr);
                            self.swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }
                    }
                }

            },
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                                                                  propagation_source: peer_id,
                                                                  message,
                                                                    message_id
                                                              })) => {
                if peer_id == self.censor_peer && self.attack_mode {
                    self.swarm.behaviour_mut().gossipsub.report_message_validation_result(&message_id, &peer_id, MessageAcceptance::Ignore);
                } else {
                    self.swarm.behaviour_mut().gossipsub.report_message_validation_result(&message_id, &peer_id, MessageAcceptance::Accept);
                }

            },
            //e => println!("{:?}",e)
            _ => ()
        }

    }

    async fn tick(timer: &mut Option<Interval>) {
        if let Some(ref mut interval) = timer {
            interval.tick().await;
        }
    }



}

fn generate_random_string(len: usize) -> String {
    let mut rng = rand::thread_rng();
    let s: String = std::iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .map(char::from)
        .take(len)
        .collect();
    s
}

