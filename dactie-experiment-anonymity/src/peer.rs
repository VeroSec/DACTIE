use crate::conn_cap::ConnCap;
use crate::link_latency::{Delayed, LatencyRange};
use crate::stem::{message_id, Route, StemAck, StemConfig, StemMessage, StemState};
use futures::StreamExt;
use libp2p::gossipsub::IdentTopic;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport};
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::{upgrade, Boxed};
use libp2p::{gossipsub, noise, plaintext, tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm, Transport};
use rand::{distributions::Alphanumeric, seq::SliceRandom, Rng};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{self, Duration, Instant, Interval};
use tokio::{io, select};

pub(crate) struct Peer {
    swarm: Swarm<MyBehaviour>,
    response_channel: UnboundedSender<PeerOut>,
    control_channel: broadcast::Receiver<ControlSignal>,
    mesh: MeshConfig,
    directory: Directory,
    /// When each connection was established, so young ones are spared when trimming.
    conn_since: HashMap<PeerId, Instant>,
    stem: StemState,
    /// Stem messages in flight, published directly if the successor fails.
    stem_pending: HashMap<OutboundRequestId, Vec<u8>>,
}

/// New connections are not trimmed during their first seconds (go-libp2p's
/// connection manager uses the same 20 s grace period).
const CONN_GRACE: Duration = Duration::from_secs(20);

/// Peer discovery of a run. Every peer registers its listen address here, and a
/// peer below the low watermark dials a uniform random sample of the others, the
/// way a discovery service hands out random node records (Ethereum's discv5).
/// The connection graph is thus a random graph, without the bias towards
/// XOR-close peers that Kademlia lookups give it, and without their bootstrap
/// traffic, which dominated the CPU of large runs.
#[derive(Clone, Default)]
pub(crate) struct Directory(Arc<RwLock<Vec<(PeerId, Multiaddr)>>>);

impl Directory {
    fn register(&self, peer: PeerId, addr: Multiaddr) {
        self.0.write().expect("directory lock").push((peer, addr));
    }

    /// Up to `k` random entries for which `skip` is false.
    fn sample(&self, k: usize, skip: impl Fn(&PeerId) -> bool) -> Vec<(PeerId, Multiaddr)> {
        let entries = self.0.read().expect("directory lock");
        entries
            .choose_multiple(&mut rand::thread_rng(), entries.len())
            .filter(|(peer, _)| !skip(peer))
            .take(k)
            .cloned()
            .collect()
    }
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
    pub(crate) outbound_min: usize,
    /// Gossip emission, the second delivery path. Each heartbeat a peer sends
    /// IHAVE to max(gossip_lazy, gossip_factor * non-mesh peers) peers it is not
    /// meshed with, and anyone lacking the message asks for it with IWANT and
    /// receives it from the announcer - the publisher included. Setting both to
    /// zero disables gossip emission, which isolates how much of the measured
    /// direct exposure comes from the mesh rather than from answering IWANTs.
    pub(crate) gossip_factor: f64,
    pub(crate) gossip_lazy: usize,
    /// Connection watermarks per peer. A peer never holds more than `conn_high`
    /// connections; once full, it closes random non-mesh ones older than the
    /// grace period down to `conn_low`, and below `conn_low` it dials random
    /// peers from the directory. Without a cap every peer connects to every
    /// other, and one process cannot hold the N^2 connections of a large network.
    pub(crate) conn_low: usize,
    pub(crate) conn_high: usize,
    /// Use the plaintext transport instead of Noise (experiments only).
    pub(crate) plaintext: bool,
    /// Gossipsub heartbeat: how often peers maintain the mesh and emit IHAVE gossip.
    pub(crate) heartbeat: Duration,
    /// Peers started per second; 0 starts them all at once. Starting in waves avoids
    /// a burst of simultaneous dials, handshakes and trims in large runs.
    pub(crate) spawn_per_sec: u32,
    /// Simulated one-way delay per connection, see `link_latency`.
    pub(crate) latency: LatencyRange,
    /// Stem phase (Dandelion++), see `stem`; off by default.
    pub(crate) stem: StemConfig,
}

impl MeshConfig {
    /// Build a band from a target degree and optional explicit bounds.
    ///
    /// gossipsub rejects a config unless both
    ///
    ///   mesh_outbound_min <= mesh_n_low <= mesh_n <= mesh_n_high
    ///   mesh_outbound_min * 2 <= mesh_n
    ///
    /// hold. `mesh_outbound_min` defaults to 2, so leaving it alone makes any D
    /// below 4 fail with MeshParametersInvalid at swarm construction - which
    /// matters here because D is swept from the command line. Deriving it from
    /// the band keeps every D constructible.
    pub(crate) fn new(
        n: usize,
        low: Option<usize>,
        high: Option<usize>,
        gossip_factor: f64,
        gossip_lazy: usize,
    ) -> Self {
        let n = n.max(1);
        let low = low
            .unwrap_or_else(|| n.saturating_sub(2).max(1))
            .clamp(1, n);
        let high = high.unwrap_or(n * 2).max(n);
        let outbound_min = (n / 2).min(low).min(2);
        MeshConfig {
            n,
            low,
            high,
            outbound_min,
            gossip_factor,
            gossip_lazy,
            conn_low: 50,
            conn_high: 100,
            plaintext: false,
            heartbeat: Duration::from_secs(1),
            spawn_per_sec: 0,
            latency: LatencyRange::OFF,
            stem: StemConfig::OFF,
        }
    }

    pub(crate) fn with_heartbeat(mut self, secs: f64) -> Self {
        self.heartbeat = Duration::from_secs_f64(secs.max(0.1));
        self
    }

    pub(crate) fn with_spawn_rate(mut self, per_sec: u32) -> Self {
        self.spawn_per_sec = per_sec;
        self
    }

    pub(crate) fn with_stem(mut self, stem: StemConfig) -> Self {
        self.stem = stem;
        self
    }

    pub(crate) fn with_link_latency(mut self, latency: LatencyRange) -> Self {
        self.latency = latency;
        self
    }

    pub(crate) fn with_plaintext(mut self, plaintext: bool) -> Self {
        self.plaintext = plaintext;
        self
    }

    pub(crate) fn with_conn_limits(mut self, low: usize, high: usize) -> Self {
        self.conn_low = low.max(1);
        self.conn_high = high.max(self.conn_low);
        self
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
    /// Received on the stem rather than through GossipSub.
    pub(crate) stem: bool,
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
    /// Network-wide background messages per second. Resolved per run into each
    /// peer's period of N / rate seconds, so the load stays the same whatever
    /// the network size.
    Rate(f64),
    /// A peer's period in seconds, resolved from `Rate`. Each peer starts at a
    /// random point within it, so publications arrive evenly rather than in bursts.
    Period(f64),
}

impl PubInterval {
    /// Resolve a network-wide rate into the per-peer period for `peer_n` peers.
    pub(crate) fn for_network(&self, peer_n: usize) -> PubInterval {
        match self {
            PubInterval::Rate(rate) => PubInterval::Period(peer_n as f64 / rate.max(f64::MIN_POSITIVE)),
            other => other.clone(),
        }
    }
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
        } else if let Some(rate) = s.strip_prefix("rate=") {
            match rate.parse::<f64>() {
                Ok(r) if r > 0.0 => Ok(PubInterval::Rate(r)),
                _ => Err(format!("Invalid rate (messages per second): '{rate}'")),
            }
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
                "Invalid interval type: '{s}'. Use 'rate=<msgs/s>', 'fix=<number>' or 'random=<start,end>'."
            ))
        }
    }
}

#[derive(NetworkBehaviour)]
pub(crate) struct MyBehaviour {
    /// The Gossipsub pub/sub behaviour is used to send broadcast messages to peers.
    pub(crate) gossipsub: gossipsub::Behaviour,
    /// Caps established connections at the high watermark, refusing before the handshake
    pub(crate) limits: ConnCap,
    /// Stem hops of the Dandelion++ phase, one request per hop.
    pub(crate) stem: request_response::json::Behaviour<StemMessage, StemAck>,
}

impl Peer {
    pub(crate) fn new(
        control_channel: broadcast::Receiver<ControlSignal>,
        response_channel: UnboundedSender<PeerOut>,
        mesh: MeshConfig,
        directory: Directory,
    ) -> Result<Peer, Box<dyn Error>> {
        Ok(Peer {
            swarm: Self::create_swarm(mesh)?,
            control_channel,
            response_channel,
            mesh,
            directory,
            conn_since: HashMap::new(),
            stem: StemState::new(mesh.stem),
            stem_pending: HashMap::new(),
        })
    }

    /// The gossipsub config for a mesh band. Separate from `create_swarm` so the
    /// parameter constraints can be checked without standing up a swarm.
    fn gossipsub_config(
        mesh: MeshConfig,
    ) -> Result<gossipsub::Config, gossipsub::ConfigBuilderError> {
        // To content-address message, we can take the hash of message and use it as an ID.
        let message_id_fn = |message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        };

        gossipsub::ConfigBuilder::default()
            .heartbeat_interval(mesh.heartbeat)
            .validation_mode(gossipsub::ValidationMode::Permissive) // This sets the kind of message validation. The default is Strict (enforce message signing)
            .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
            .flood_publish(false)
            // D is the quantity the anonymity bound is stated over, so the whole
            // band is configurable rather than fixed.
            .mesh_n_low(mesh.low)
            .mesh_n(mesh.n)
            .mesh_n_high(mesh.high)
            .mesh_outbound_min(mesh.outbound_min)
            .gossip_factor(mesh.gossip_factor)
            .gossip_lazy(mesh.gossip_lazy)
            .build()
    }

    fn create_swarm(mesh: MeshConfig) -> Result<Swarm<MyBehaviour>, Box<dyn Error>> {
        let builder = libp2p::SwarmBuilder::with_new_identity().with_tokio();
        let swarm_config =
            |c: libp2p::swarm::Config| c.with_idle_connection_timeout(Duration::from_secs(60));
        // Plaintext skips Noise, whose per-connection buffers dominate memory in
        // large single-process runs. Encryption does not affect who receives what
        // from whom and when, which is all the experiment measures.
        let mut swarm = if !mesh.latency.is_off() {
            builder
                .with_other_transport(|key| Self::delayed_transport(key, mesh))?
                .with_behaviour(|_| Self::build_behaviour(mesh))?
                .with_swarm_config(swarm_config)
                .build()
        } else if mesh.plaintext {
            builder
                .with_tcp(
                    tcp::Config::default(),
                    |key: &libp2p::identity::Keypair| {
                        Ok::<_, std::convert::Infallible>(plaintext::Config::new(key))
                    },
                    yamux::Config::default,
                )?
                .with_behaviour(|_| Self::build_behaviour(mesh))?
                .with_swarm_config(swarm_config)
                .build()
        } else {
            builder
                .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
                .with_behaviour(|_| Self::build_behaviour(mesh))?
                .with_swarm_config(swarm_config)
                .build()
        };

        // Loopback only: every peer runs in this process. Listening on all
        // interfaces made each peer advertise one address per interface, and every
        // dial then opened that many sockets and aborted all but one mid-handshake.
        swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse()?)?;

        Ok(swarm)
    }

    /// TCP whose connections lag by the simulated link delay, below the
    /// security handshake so that handshakes take their round trips too.
    fn delayed_transport(
        key: &libp2p::identity::Keypair,
        mesh: MeshConfig,
    ) -> Result<Boxed<(PeerId, StreamMuxerBox)>, Box<dyn Error + Send + Sync>> {
        let range = mesh.latency;
        // Port reuse binds outgoing connections to the listening port, so both ends
        // of a link see both listening ports and derive the same delay. Nagle is off
        // (libp2p's nodelay(false) default, commented "Disable Nagle's algorithm",
        // enables it): the kernel acknowledges loopback segments without the
        // simulated delay, so Nagle's wait for acknowledgements held messages on busy
        // links for up to the 40 ms delayed-ACK timeout, which no simulated link
        // reflects, and relayed copies overtook the direct one.
        let tcp = tcp::tokio::Transport::new(
            tcp::Config::default().port_reuse(true).nodelay(true),
        );
        let tcp = tcp.map(move |stream, _| {
            let ports = (stream.0.local_addr(), stream.0.peer_addr());
            let delay = match ports {
                (Ok(local), Ok(peer)) => range.for_ports(local.port(), peer.port()),
                _ => Duration::from_millis(range.max_ms),
            };
            Delayed::new(stream, delay)
        });
        let upgraded = tcp.upgrade(upgrade::Version::V1);
        Ok(if mesh.plaintext {
            upgraded
                .authenticate(plaintext::Config::new(key))
                .multiplex(yamux::Config::default())
                .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
                .boxed()
        } else {
            upgraded
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default())
                .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
                .boxed()
        })
    }

    fn build_behaviour(mesh: MeshConfig) -> Result<MyBehaviour, Box<dyn Error + Send + Sync>> {
        let gossipsub_config = Self::gossipsub_config(mesh).map_err(io::Error::other)?;

        let mut gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Anonymous,
            gossipsub_config,
        )?;

        gossipsub.subscribe(&IdentTopic::new("Test"))?;

        let stem = request_response::json::Behaviour::new(
            [(StreamProtocol::new("/dactie/stem/1"), ProtocolSupport::Full)],
            request_response::Config::default(),
        );

        Ok(MyBehaviour {
            gossipsub,
            limits: ConnCap::new(Some(mesh.conn_high)),
            stem,
        })
    }

    pub(crate) fn get_peer_id(&mut self) -> PeerId {
        *self.swarm.local_peer_id()
    }

    pub(crate) async fn run(mut self, pub_interval: PubInterval) {
        let mut timer: Option<Interval> = None;
        let mut trim = time::interval(Duration::from_secs(5));
        loop {
            select! {
                _ = trim.tick() => {
                    self.trim_connections();
                    self.top_up();
                    for data in self.stem.due(Instant::now()) {
                        self.fluff(data);
                    }
                }
                Ok(signal) = self.control_channel.recv() => {
                    match signal {
                        ControlSignal::Run => {
                            if timer.is_none() {
                                timer = Some(match pub_interval {
                                    PubInterval::Fix(val) => time::interval(Duration::from_secs(val)),
                                    PubInterval::Random((min, max)) => time::interval(Duration::from_secs(
                                        rand::thread_rng().gen_range(min..=max).try_into().unwrap(),
                                    )),
                                    PubInterval::Period(secs) => {
                                        let period = Duration::from_secs_f64(secs.max(0.001));
                                        let phase = period.mul_f64(rand::thread_rng().gen::<f64>());
                                        time::interval_at(Instant::now() + phase, period)
                                    }
                                    // Resolved into Period by the experiment before peers start
                                    PubInterval::Rate(_) => unreachable!("Rate is resolved per run"),
                                });
                            }
                        }
                        ControlSignal::Stop => {
                            break
                        },
                        ControlSignal::Pub(val) => {
                            // Every peer reports its connections and mesh peers, so a run's
                            // log shows whether the graph held 50 to 100 connections per peer.
                            let topic = IdentTopic::new("Test");
                            let connected = self.swarm.connected_peers().count();
                            let mesh = self.swarm.behaviour().gossipsub.mesh_peers(&topic.hash()).count();
                            println!("peer_state {connected} {mesh}");
                            if val == self.get_peer_id() {
                                println!("Stopping");
                                let message = "Stop".repeat(30);
                                let result = self.publish_own(message.into_bytes());
                                println!(
                                    "Publisher state: {connected} connections, {mesh} mesh peers, publish {result}"
                                );
                            }
                        }
                    }

                },
                _ = Self::tick(&mut timer), if timer.is_some() => {

                    let message = generate_random_string(120);
                    self.publish_own(message.into_bytes());
                },
                event = self.swarm.select_next_some() => self.handle_event(event)
            }
        }
    }

    /// Once a peer reaches the high watermark, close connections down to the low
    /// watermark so that newcomers find free slots. Gossipsub mesh peers and
    /// connections younger than the grace period are kept.
    fn trim_connections(&mut self) {
        let connected: Vec<PeerId> = self.swarm.connected_peers().copied().collect();
        if connected.len() < self.mesh.conn_high {
            return;
        }
        let protected: HashSet<PeerId> =
            self.swarm.behaviour().gossipsub.all_mesh_peers().copied().collect();
        let now = Instant::now();
        let mut candidates: Vec<PeerId> = connected
            .iter()
            .filter(|p| !protected.contains(*p))
            .filter(|p| {
                self.conn_since
                    .get(*p)
                    .is_none_or(|since| now.duration_since(*since) >= CONN_GRACE)
            })
            .copied()
            .collect();
        candidates.shuffle(&mut rand::thread_rng());
        let excess = connected.len() - self.mesh.conn_low;
        for peer in candidates.into_iter().take(excess) {
            let _ = self.swarm.disconnect_peer_id(peer);
        }
    }

    /// Below the low watermark, dial random peers from the directory to make up
    /// the difference. Dials to full peers are refused before the handshake and
    /// retried with a fresh sample on the next tick.
    fn top_up(&mut self) {
        let connected = self.swarm.connected_peers().count();
        if connected >= self.mesh.conn_low {
            return;
        }
        let me = *self.swarm.local_peer_id();
        let sample = self.directory.sample(self.mesh.conn_low - connected, |peer| {
            *peer == me || self.swarm.is_connected(peer)
        });
        for (peer, addr) in sample {
            let _ = self.swarm.dial(
                DialOpts::peer_id(peer)
                    .addresses(vec![addr])
                    .condition(PeerCondition::DisconnectedAndNotDialing)
                    .build(),
            );
        }
    }

    /// Publish one of this peer's own messages: into the stem when it is on,
    /// otherwise straight into GossipSub. Returns what happened, for the log.
    fn publish_own(&mut self, data: Vec<u8>) -> String {
        if !self.stem.is_on() {
            return self.fluff(data);
        }
        let id = message_id(&data);
        self.stem.first_time(id);
        self.stem.mark_own(id);
        self.stem_step(None, id, data)
    }

    /// Forward a stem message to the next hop, or fluff it here.
    fn stem_step(&mut self, from: Option<PeerId>, id: u64, data: Vec<u8>) -> String {
        let connected: Vec<PeerId> = self.swarm.connected_peers().copied().collect();
        let now = Instant::now();
        let mut rng = rand::thread_rng();
        match self.stem.route(from, &connected, now, &mut rng) {
            Route::Forward(next) => {
                self.send_stem(next, id, data);
                "stem".to_string()
            }
            Route::Fluff => self.fluff(data),
        }
    }

    fn send_stem(&mut self, next: PeerId, id: u64, data: Vec<u8>) {
        self.stem
            .arm_embargo(id, data.clone(), Instant::now(), &mut rand::thread_rng());
        let request = self
            .swarm
            .behaviour_mut()
            .stem
            .send_request(&next, StemMessage { data: data.clone() });
        self.stem_pending.insert(request, data);
    }

    /// Re-route this peer's own message away from `avoid`; without a new hop it
    /// waits for the embargo. Returns false if the message is not its own.
    fn retry_own(&mut self, id: u64, data: &[u8], avoid: &[PeerId]) -> bool {
        if !self.stem.is_own(id) {
            return false;
        }
        let connected: Vec<PeerId> = self.swarm.connected_peers().copied().collect();
        if let Some(next) =
            self.stem
                .retry_own(id, avoid, &connected, Instant::now(), &mut rand::thread_rng())
        {
            self.send_stem(next, id, data.to_vec());
        }
        true
    }

    /// A stem message arrived from `from`.
    fn on_stem(&mut self, from: PeerId, data: Vec<u8>) {
        let id = message_id(&data);
        if !self.stem.first_time(id) {
            // The stem looped back here. The source re-routes its own message;
            // a relay spreads it now instead of waiting for the embargo, which
            // reveals only the relay.
            let avoid: Vec<PeerId> = [Some(from), self.stem.own_successor()].into_iter().flatten().collect();
            if !self.retry_own(id, &data, &avoid) {
                self.fluff(data);
            }
            return;
        }
        if is_measurement(&data) {
            self.report(from, true);
        }
        self.stem_step(Some(from), id, data);
    }

    /// Publish into GossipSub. A duplicate means the message is already out.
    fn fluff(&mut self, data: Vec<u8>) -> String {
        match self.swarm.behaviour_mut().gossipsub.publish(IdentTopic::new("Test"), data) {
            Ok(_) => "ok".to_string(),
            Err(gossipsub::PublishError::Duplicate) => "duplicate".to_string(),
            Err(e) => format!("failed: {e:?}"),
        }
    }

    fn report(&self, prop_id: PeerId, stem: bool) {
        self.response_channel
            .send(PeerOut {
                observer: *self.swarm.local_peer_id(),
                prop_id,
                at: Instant::now(),
                stem,
            })
            .unwrap();
    }

    fn handle_event(&mut self, event: SwarmEvent<MyBehaviourEvent>) {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                self.conn_since.entry(peer_id).or_insert_with(Instant::now);
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
                self.conn_since.remove(&peer_id);
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                let me = *self.swarm.local_peer_id();
                self.directory.register(me, address);
                self.top_up();
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source: peer_id,
                message,
                ..
            })) => {
                self.stem.seen_in_gossip(message_id(&message.data));
                if is_measurement(&message.data) {
                    self.report(peer_id, false);
                }
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Stem(request_response::Event::Message {
                peer,
                message,
            })) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let _ = self.swarm.behaviour_mut().stem.send_response(channel, StemAck);
                    self.on_stem(peer, request.data);
                }
                request_response::Message::Response { request_id, .. } => {
                    self.stem_pending.remove(&request_id);
                }
            },
            SwarmEvent::Behaviour(MyBehaviourEvent::Stem(request_response::Event::OutboundFailure {
                peer,
                request_id,
                ..
            })) => {
                // The successor did not take it. The source tries another hop;
                // a relay spreads it from here.
                if let Some(data) = self.stem_pending.remove(&request_id) {
                    if !self.retry_own(message_id(&data), &data, &[peer]) {
                        self.fluff(data);
                    }
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

/// The measurement message, as opposed to background traffic.
fn is_measurement(data: &[u8]) -> bool {
    data.starts_with(b"Stop")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The directory never hands out skipped peers (self, already connected) and
    /// returns fewer than asked once the others run out.
    #[test]
    fn directory_samples_only_unskipped_peers() {
        let directory = Directory::default();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1".parse().unwrap();
        let peers: Vec<PeerId> = (0..10).map(|_| PeerId::random()).collect();
        for peer in &peers {
            directory.register(*peer, addr.clone());
        }
        let skip: HashSet<PeerId> = peers[..4].iter().copied().collect();
        let sample = directory.sample(5, |p| skip.contains(p));
        assert_eq!(sample.len(), 5);
        assert!(sample.iter().all(|(p, _)| !skip.contains(p)));
        assert_eq!(directory.sample(100, |p| skip.contains(p)).len(), 6);
    }

    /// Every D the command line can ask for must produce a config gossipsub
    /// accepts. D=2 previously failed with MeshParametersInvalid, because
    /// mesh_outbound_min defaults to 2 and the band left it untouched.
    #[test]
    fn every_mesh_degree_builds() {
        for n in 1..=64usize {
            let mesh = MeshConfig::new(n, None, None, 0.25, 6);
            assert!(
                Peer::gossipsub_config(mesh).is_ok(),
                "D={n} rejected: {mesh:?}"
            );
        }
    }

    /// A pinned band, which is what a bound-validation run uses, must build too.
    #[test]
    fn a_pinned_band_builds() {
        for n in 1..=64usize {
            let mesh = MeshConfig::new(n, Some(n), Some(n), 0.25, 6);
            assert_eq!(mesh.low, n);
            assert_eq!(mesh.high, n);
            assert!(
                Peer::gossipsub_config(mesh).is_ok(),
                "pinned D={n} rejected: {mesh:?}"
            );
        }
    }

    /// The invariants gossipsub checks, asserted directly so a regression names
    /// the broken relation rather than surfacing as a panic in swarm setup.
    #[test]
    fn band_satisfies_gossipsub_invariants() {
        for n in 1..=64usize {
            for low in [None, Some(1), Some(n), Some(n * 2)] {
                for high in [None, Some(n), Some(n + 1)] {
                    let m = MeshConfig::new(n, low, high, 0.25, 6);
                    assert!(m.outbound_min <= m.low, "{m:?}");
                    assert!(m.low <= m.n, "{m:?}");
                    assert!(m.n <= m.high, "{m:?}");
                    assert!(m.outbound_min * 2 <= m.n, "{m:?}");
                }
            }
        }
    }
}

#[cfg(test)]
mod pub_interval_tests {
    use super::PubInterval;
    use std::str::FromStr;

    #[test]
    fn network_rate_resolves_to_per_peer_period() {
        let rate = PubInterval::from_str("rate=40").unwrap();
        match rate.for_network(800) {
            PubInterval::Period(secs) => assert!((secs - 20.0).abs() < 1e-9),
            other => panic!("expected a period, got {other:?}"),
        }
        match rate.for_network(50) {
            PubInterval::Period(secs) => assert!((secs - 1.25).abs() < 1e-9),
            other => panic!("expected a period, got {other:?}"),
        }
    }

    #[test]
    fn fixed_intervals_are_left_alone_and_bad_rates_rejected() {
        assert!(matches!(PubInterval::from_str("fix=3").unwrap().for_network(800), PubInterval::Fix(3)));
        assert!(PubInterval::from_str("rate=0").is_err());
        assert!(PubInterval::from_str("rate=abc").is_err());
    }
}
