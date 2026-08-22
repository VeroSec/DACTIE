use crate::adversary::Observation;
use crate::peer::{ControlSignal, MeshConfig, Peer, PubInterval};
use futures::future::join_all;
use libp2p::PeerId;
use std::collections::HashSet;
use std::error::Error;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep, Duration, Instant};

/// Everything one run yields. The first three fields are the original
/// aggregates, kept so the existing CSV is unchanged; the rest is the raw
/// record an adversary would have, which the attack post-processes.
pub(crate) struct RunOutcome {
    pub(crate) correct_receiver: f32,
    pub(crate) other_peer: f32,
    pub(crate) distinct_prop_peers: usize,
    pub(crate) observations: Vec<Observation>,
    pub(crate) source: PeerId,
    pub(crate) all_peers: Vec<PeerId>,
}

impl RunOutcome {
    /// Fraction of the peers that should have received the measurement message
    /// and did. Every peer but the source is expected to, so a healthy run is
    /// exactly 1.0.
    ///
    /// This is a validity check, not a result. A run that only reaches part of
    /// the network is a different experiment: `correct_receiver` starts
    /// counting peers the publisher answered an IWANT for rather than peers in
    /// its mesh, so it can exceed D and push the measured value above a bound
    /// derived from D. Meanwhile the attack samples colluders from all peers
    /// while only the reached ones carry observations, so attacker success is
    /// diluted. Both distortions are silent in the aggregates.
    pub(crate) fn delivery_ratio(&self) -> f32 {
        let expected = self.all_peers.len().saturating_sub(1);
        if expected == 0 {
            return 1.0;
        }
        (self.correct_receiver + self.other_peer) / expected as f32
    }
}

//Creates n peers and lets them exchange messages on one topic. Each peer publishes in a random interval between 3 and 10 seconds messages.
// After time before stop a single peer sends a measurement message. The other peers report the PeerID of the peer that propagated the message.
//Returns (no peers where publishing peer == propagating peers, other peers, number of non duplicate propagating peers )
pub(crate) async fn experiment_prop_peer_id(
    peer_n: usize,
    time_before_stop: Duration,
    pub_interval: PubInterval,
    mesh: MeshConfig,
) -> Result<RunOutcome, Box<dyn Error>> {
    let mut peers = Vec::new();
    let mut all_peers = Vec::new();
    let (control_tx, control_rx) = broadcast::channel::<ControlSignal>(peer_n);
    let (response_tx, mut response_rx) = mpsc::unbounded_channel();

    let mut boot_peer = Peer::new_bootstrap_peer(control_rx, response_tx.clone(), mesh)?;

    let boot_peer_id = boot_peer.get_peer_id();
    all_peers.push(boot_peer_id);

    let mut pub_interval_clone = pub_interval.clone();

    // Run the peer in its own asynchronous task
    let boot_peer_task = tokio::spawn(async move {
        boot_peer.run(pub_interval_clone).await;
    });

    peers.push(boot_peer_task);

    sleep(Duration::from_secs(2)).await;

    for _ in 0..peer_n - 2 {
        let mut peer = Peer::new_peer(
            boot_peer_id,
            control_tx.subscribe(),
            response_tx.clone(),
            mesh,
        )?;
        all_peers.push(peer.get_peer_id());
        pub_interval_clone = pub_interval.clone();

        // Run the peer in its own asynchronous task
        let peer_task = tokio::spawn(async move {
            peer.run(pub_interval_clone).await;
        });
        peers.push(peer_task);
    }

    let mut stop_peer = Peer::new_peer(
        boot_peer_id,
        control_tx.subscribe(),
        response_tx.clone(),
        mesh,
    )?;

    let stop_peer_addr = stop_peer.get_peer_id();
    all_peers.push(stop_peer_addr);

    let mut interval = tokio::time::interval(time_before_stop);
    interval.tick().await; //inverals first tick is immediately.
                           // Run the peer in its own asynchronous task
    let peer_task = tokio::spawn(async move {
        stop_peer.run(pub_interval).await;
    });
    peers.push(peer_task);

    control_tx.send(ControlSignal::Run).unwrap();

    interval.tick().await;

    let published_at = Instant::now();
    control_tx.send(ControlSignal::Pub(stop_peer_addr)).unwrap();

    tokio::time::sleep(Duration::from_secs(30)).await;

    control_tx.send(ControlSignal::Stop).unwrap();

    let mut correct_receiver = 0f32;
    let mut other_peer = 0f32;
    let mut duplicate_free_propagation_peers = HashSet::new();
    join_all(peers).await;
    println!("peers stopped");

    drop(response_tx);

    let mut observations = Vec::new();

    while let Some(val) = response_rx.recv().await {
        duplicate_free_propagation_peers.insert(val.prop_id);
        if val.prop_id == stop_peer_addr {
            correct_receiver += 1f32;
        } else {
            other_peer += 1f32;
        }
        // saturating: an observation cannot predate the publish, but the peers
        // stamp their own arrivals and we do not want a panic if one races.
        observations.push(Observation {
            observer: val.observer,
            prop_id: val.prop_id,
            at: val.at.saturating_duration_since(published_at),
        });
    }

    Ok(RunOutcome {
        correct_receiver,
        other_peer,
        distinct_prop_peers: duplicate_free_propagation_peers.len(),
        observations,
        source: stop_peer_addr,
        all_peers,
    })
}
