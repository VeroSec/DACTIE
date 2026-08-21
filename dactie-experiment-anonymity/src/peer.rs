use futures::StreamExt;
use libp2p::gossipsub::IdentTopic;
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{Mode, PROTOCOL_NAME};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identify, kad, noise, tcp, yamux, Multiaddr, PeerId, Swarm};
use rand::{distributions::Alphanumeric, Rng};
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{self, Duration, Instant, Interval};
use tokio::{io, select};

pub(crate) struct Peer {
    swarm: Swarm<MyBehaviour>,
    response_channel: UnboundedSender<PeerOut>,
    control_channel: broadcast::Receiver<ControlSignal>,
}

/// The gossipsub mesh band a run is configured with.
///
/// D is the quantity the anonymity bound is stated over, so `n` is swept rather
/// than fixed. `low` and `high` are the band gossipsub is allowed to drift
/// within between heartbeats; for bound-validation runs pin `high` close to `n`,
/// since a mesh that drifts to 2*D is no longer the D the bound is stated over.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeshConfig {
    pub(crate) n: usize,
    pub(crate) low: usize,
    pub(crate) high: usize,
}

impl MeshConfig {
    /// Build a band from a target degree and optional explicit bounds, keeping
    /// gossipsub's `low <= n <= high` invariant so `ConfigBuilder` cannot fail.
    pub(crate) fn new(n: usize, low: Option<usize>, high: Option<usize>) -> Self {
        let n = n.max(1);
        let low = low
            .unwrap_or_else(|| n.saturating_sub(2).max(1))
            .clamp(1, n);
        let high = high.unwrap_or(n * 2).max(n);
        MeshConfig { n, low, high }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PeerOut {
    /// Which peer made this observation. A colluding subset can only pool what
    /// its own members saw, so without this the data cannot be partitioned.
    pub(crate) observer: PeerId,
    /// PeerId of the propagation_source: who relayed the message to `observer`.
    /// This is the relaying neighbour, not the originator.
    pub(crate) prop_id: PeerId,
    /// When the observation was made. Converted to a publish-relative Duration
    /// by the experiment, which is the only place that knows the publish time.
    pub(crate) at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) enum ControlSignal {
    Run,
    Stop,
    Pub(PeerId),
}

#[derive(Clone, Debug)]
pub(crate) enum PubInterval {
    Fix(u64), //in seconds
    Random((usize, usize)),
}

// Implement FromStr for PubInterval to handle argument parsing
impl FromStr for PubInterval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(fix_value) = s.strip_prefix("fix=") {
            fix_value
                .parse::<u64>()
                .map(PubInterval::Fix)
                .map_err(|_| format!("Invalid number for Fix: '{fix_value}'"))
        } else if let Some(random_values) = s.strip_prefix("random=") {
            let parts: Vec<&str> = random_values.split(',').collect();
            if parts.len() == 2 {
                let start = parts[0]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid number: '{}'", parts[0]))?;
                let end = parts[1]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid number: '{}'", parts[1]))?;
                Ok(PubInterval::Random((start, end)))
            } else {
                Err(format!("Invalid format for Random: '{random_values}'"))
            }
        } else {
            Err(format!(
                "Invalid interval type: '{s}'. Use 'fix=<number>' or 'random=<start,end>'."
            ))
        }
    }
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
    pub(crate) fn new_bootstrap_peer(
        control_channel: broadcast::Receiver<ControlSignal>,
        response_channel: UnboundedSender<PeerOut>,
        mesh: MeshConfig,
    ) -> Result<Peer, Box<dyn Error>> {
        let swarm = Self::create_swarm(None, mesh)?;

        Ok(Peer {
            swarm,
            control_channel,
            response_channel,
        })
    }

    pub(crate) fn new_peer(
        boot_peer_id: PeerId,
        control_channel: broadcast::Receiver<ControlSignal>,
        response_channel: UnboundedSender<PeerOut>,
        mesh: MeshConfig,
    ) -> Result<Peer, Box<dyn Error>> {
        let swarm = Self::create_swarm(
            Some((boot_peer_id, "/ip4/127.0.0.1/tcp/10001".parse()?)),
            mesh,
        )?;

        Ok(Peer {
            swarm,
            control_channel,
            response_channel,
        })
    }

    fn create_swarm(
        bootstrap_peer: Option<(PeerId, Multiaddr)>,
        mesh: MeshConfig,
    ) -> Result<Swarm<MyBehaviour>, Box<dyn Error>> {
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
                    .heartbeat_interval(Duration::from_secs(1)) // This is set to aid debugging by not cluttering the log space
                    .validation_mode(gossipsub::ValidationMode::Permissive) // This sets the kind of message validation. The default is Strict (enforce message signing)
                    .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
                    .flood_publish(false)
                    // D is the quantity the anonymity bound is stated over, so
                    // the whole band is configurable rather than fixed.
                    .mesh_n_low(mesh.low)
                    .mesh_n(mesh.n)
                    .mesh_n_high(mesh.high)
                    .build()
                    .map_err(io::Error::other)
                    .unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.

                let mut gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Anonymous,
                    gossipsub_config,
                )?;

                gossipsub.subscribe(&IdentTopic::new("Test"))?;

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

        if bootstrap_peer.is_none() {
            swarm.listen_on("/ip4/0.0.0.0/tcp/10001".parse()?)?;
        } else {
            swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        }

        Ok(swarm)
    }

    pub(crate) fn get_peer_id(&mut self) -> PeerId {
        *self.swarm.local_peer_id()
    }

    pub(crate) async fn run(mut self, pub_interval: PubInterval) {
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
                        ControlSignal::Pub(val) => {
                            if val == self.get_peer_id(){
                                 println!("Stopping");
                                let message = "Stop".repeat(30);
                                let _ = self.swarm.behaviour_mut().gossipsub.publish(IdentTopic::new("Test"),message); //Not handled
                            }
                        }
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

    fn handle_event(&mut self, event: SwarmEvent<MyBehaviourEvent>) {
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
                    if protocols.contains(&PROTOCOL_NAME) {
                        for addr in listen_addrs {
                            log::debug!("Adding received IdentifyInfo matching protocol  to the DHT. Peer: {}, addr: {}", peer_id, addr);
                            self.swarm
                                .behaviour_mut()
                                .kademlia
                                .add_address(&peer_id, addr);
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source: peer_id,
                message,
                ..
            })) => {
                if String::from_utf8_lossy(&message.data[..4]) == "Stop" {
                    let observer = *self.swarm.local_peer_id();
                    self.response_channel
                        .send(PeerOut {
                            observer,
                            prop_id: peer_id,
                            at: Instant::now(),
                        })
                        .unwrap();
                }
            }
            //e => println!("{:?}",e)
            _ => (),
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
