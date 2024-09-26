use std::collections::HashSet;
use std::error::Error;
use futures::future::join_all;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep,Duration};
use crate::peer::{ControlSignal, Peer, PubInterval};




//Creates n peers and lets them exchange messages on one topic. Each peer publishes in a random interval between 3 and 10 seconds messages.
// After time before stop a single peer sends a measurement message. The other peers report the PeerID of the peer that propagated the message.
//Returns (no peers where publishing peer == propagating peers, other peers, number of non duplicate propagating peers )
pub(crate) async fn experiment_prop_peer_id(peer_n: usize, time_before_stop: Duration, pub_interval: PubInterval) -> Result<(f32,f32, usize), Box<dyn Error>> {
    let mut peers = Vec::new();
    let (control_tx, control_rx) = broadcast::channel::<ControlSignal>(peer_n);
    let (response_tx, mut response_rx) = mpsc::unbounded_channel();

    let mut boot_peer = Peer::new_bootstrap_peer(control_rx, response_tx.clone())?;

    let boot_peer_id = boot_peer.get_peer_id();

    let mut pub_interval_clone= pub_interval.clone();

    // Run the peer in its own asynchronous task
    let boot_peer_task = tokio::spawn(async move {
        boot_peer.run(pub_interval_clone).await;
    });

    peers.push(boot_peer_task);

    sleep(Duration::from_secs(2)).await;


    for _ in 0..peer_n -2{
        let peer = Peer::new_peer(boot_peer_id,control_tx.subscribe(), response_tx.clone())?;
        pub_interval_clone = pub_interval.clone();

        // Run the peer in its own asynchronous task
        let peer_task = tokio::spawn(async move {
            peer.run(pub_interval_clone).await;
        });
        peers.push(peer_task);
    }

    let mut stop_peer = Peer::new_peer(boot_peer_id, control_tx.subscribe(), response_tx.clone())?;

    let stop_peer_addr = stop_peer.get_peer_id();


    let mut interval = tokio::time::interval(time_before_stop);
    interval.tick().await; //inverals first tick is immediately.
    // Run the peer in its own asynchronous task
    let peer_task = tokio::spawn(async move {
        stop_peer.run(pub_interval).await;
    });
    peers.push(peer_task);



    control_tx.send(ControlSignal::Run).unwrap();

    interval.tick().await;

    control_tx.send(ControlSignal::Pub(stop_peer_addr)).unwrap();

    tokio::time::sleep(Duration::from_secs(30)).await;

    control_tx.send(ControlSignal::Stop).unwrap();


    let mut correct_receiver = 0f32;
    let mut other_peer = 0f32;
    let mut duplicate_free_propagation_peers = HashSet::new();
    join_all(peers).await;
    println!("peers stopped");

    drop(response_tx);


    while let Some(val) = response_rx.recv().await {
        duplicate_free_propagation_peers.insert(val.prop_id);
        if val.prop_id == stop_peer_addr {
            correct_receiver += 1f32;
        } else {
            other_peer +=1f32;
        }
    }

    Ok((correct_receiver, other_peer, duplicate_free_propagation_peers.len() ))




}