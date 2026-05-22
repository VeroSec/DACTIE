use std::collections::HashSet;
use std::error::Error;
use futures::future::join_all;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep,Duration};
use crate::bad_peer;
use crate::peer;
use crate::shared_structs::*;




//Creates n honest peers and attack_peer_n attacker peers and lets them exchange messages on one topic. Each peer publishes in a random interval between 3 and 10 seconds messages.
// After time_before attack the attacker peers stop forwarding messages of a victim peer. 
// After time_before stop the victim sends a message to all other peers. All peers that receive the victims message report back to this function
//Returns (amount of peers that received the victims message)
pub(crate) async fn experiment_prop_peer_id(peer_n: usize, attack_peer_n: usize, time_before_attack: Duration, time_before_stop: Duration, pub_interval: PubInterval) -> Result<f32, Box<dyn Error>> {
    let mut peers = Vec::new();
    let (control_tx, control_rx) = broadcast::channel::<ControlSignal>(peer_n);
    let (response_tx, mut response_rx) = mpsc::unbounded_channel();

    let mut boot_peer = peer::Peer::new_bootstrap_peer(control_rx, response_tx.clone())?;

    let boot_peer_id = boot_peer.get_peer_id();

    let mut pub_interval_clone= pub_interval.clone();

    // Run the peer in its own asynchronous task
    let boot_peer_task = tokio::spawn(async move {
        boot_peer.run(pub_interval_clone).await;
    });

    peers.push(boot_peer_task);

    sleep(Duration::from_secs(2)).await;


    for _ in 0..peer_n -2{
        let peer = peer::Peer::new_peer(boot_peer_id,control_tx.subscribe(), response_tx.clone())?;
        pub_interval_clone = pub_interval.clone();

        // Run the peer in its own asynchronous task
        let peer_task = tokio::spawn(async move {
            peer.run(pub_interval_clone).await;
        });
        peers.push(peer_task);
    }



    let mut stop_peer = peer::Peer::new_peer(boot_peer_id, control_tx.subscribe(), response_tx.clone())?;

    let stop_peer_addr = stop_peer.get_peer_id();



    for _ in 0..attack_peer_n{
        let peer = bad_peer::Peer::new_peer(boot_peer_id,control_tx.subscribe(), response_tx.clone(), stop_peer_addr)?;
        pub_interval_clone = pub_interval.clone();

        // Run the peer in its own asynchronous task
        let peer_task = tokio::spawn(async move {
            peer.run(pub_interval_clone).await;
        });
        peers.push(peer_task);
    }

    sleep(Duration::from_secs(10)).await;


    let mut attack_interval = tokio::time::interval(time_before_attack);
    attack_interval.tick().await; //inverals first tick is immediately.
    let mut stop_interval = tokio::time::interval(time_before_stop);
    stop_interval.tick().await; //inverals first tick is immediately.
    // Run the peer in its own asynchronous task
    let peer_task = tokio::spawn(async move {
        stop_peer.run(pub_interval).await;
    });
    peers.push(peer_task);


    println!("Sending Control Signal");
    control_tx.send(ControlSignal::Run).unwrap();

    attack_interval.tick().await;

    println!("Sending Attack Signal");
    control_tx.send(ControlSignal::Attack).unwrap();

    stop_interval.tick().await;

    println!("Sending Publish Signal");
    control_tx.send(ControlSignal::Pub(stop_peer_addr)).unwrap();

    tokio::time::sleep(Duration::from_secs(30)).await;

    control_tx.send(ControlSignal::Stop).unwrap();


    let mut receivers = 0f32;
    join_all(peers).await;
    println!("peers stopped");

    drop(response_tx);


    while let Some(val) = response_rx.recv().await {
        receivers += 1f32;

    }

    Ok(receivers)




}