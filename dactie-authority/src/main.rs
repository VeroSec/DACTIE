use crate::abstswarm::abstswarm_builder::AbstSwarmBuilder;
use crate::mybehaviour::MyBehaviour;
use crate::myclient::MyClient;
use crate::storage::Storage;
use clap::Parser;
use dactie_utils::key_store::StoredKeyMaterial;
use dactie_utils::mls_wrapper::MyOpenMls;
use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use libp2p::Multiaddr;

mod abstswarm;
mod mybehaviour;
mod myclient;
mod storage;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Location of the folder, where the keymaterial should be saved
    #[arg(short = 'K', long)]
    km_dir: String,

    /// Port to run service on
    #[arg(short, long)]
    port: Option<usize>,

    /// Not used: groups and subscribed topics are restored from --km-dir
    #[arg(short, long)]
    load_file: Option<String>,

    /// Explicit external address to advertise (e.g. /ip4/10.244.192.116/tcp/10001)
    #[arg(long)]
    advertise_addr: Option<Multiaddr>,

    /// Archives needed to open a signature. Fixed when the archive init phase ends;
    /// defaults to a strict majority of the registered archives.
    #[arg(long)]
    threshold: Option<usize>,

    /// Length of an epoch in seconds. A sharing peer's registration lasts until the end of
    /// the epoch of its slot tag.
    #[arg(long, default_value_t = 30 * 24 * 3600)]
    epoch_secs: u64,

    /// Slots per member and epoch (k): how many sharing peers a member can register.
    #[arg(long, default_value_t = 5)]
    peers_per_member: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    // Handle Sigterm for Docker
    tokio::spawn(async move {
        let mut term_signal =
            signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal handler");
        term_signal.recv().await;
        println!("Received SIGTERM, shutting down gracefully...");
        exit(0);
    });

    let args = Args::parse();

    //Load Save-File
    let mut km_path = PathBuf::from_str(&args.km_dir).unwrap();
    km_path.push("key_material.json");
    let mut store_path = PathBuf::from_str(&args.km_dir).unwrap();
    store_path.push("storage.json");
    let (storage, key_material) = if km_path.exists() && store_path.exists() {
        let storage = Storage::from_file(store_path)?;
        let key_material = StoredKeyMaterial::from_file(km_path)?;
        (storage, key_material)
    } else {
        let (storage, key_material) = Storage::new(store_path);
        storage.to_file()?;
        key_material.to_file(km_path)?;
        (storage, key_material)
    };

    // Deployments whose init phase ended before the threshold was stored created every
    // member's shares with half the archives (rounded down), so keep using that.
    if !storage.get_archive_init_mode() && storage.get_threshold().is_none() {
        let legacy = (storage.get_archive_peer_ids().len() / 2).max(1);
        log::warn!("No opening threshold stored; using the legacy value {legacy}");
        storage.set_threshold(legacy);
        storage.to_file()?;
    }
    if let Some(threshold) = args.threshold {
        match storage.get_threshold() {
            Some(fixed) if !storage.get_archive_init_mode() && fixed != threshold => log::warn!(
                "--threshold {threshold} ignored: the init phase already fixed the threshold at {fixed}"
            ),
            _ if storage.get_archive_init_mode() => storage.set_threshold(threshold),
            _ => {}
        }
    }

    storage.set_epoch_policy(args.epoch_secs, args.peers_per_member);

    log::info!(
        "Archives registered: {}, init phase: {}",
        storage.get_archive_peer_ids().len(),
        if storage.get_archive_init_mode() {
            "open"
        } else {
            "CLOSED"
        }
    );

    let (notification_tx, notification_rx) = mpsc::unbounded_channel();
    let (instruction_tx, instruction_rx) = mpsc::unbounded_channel();
    let (group_commit_tx, group_commit_rx) = mpsc::unbounded_channel();

    let load_file_path = PathBuf::from_str(&args.km_dir)?;

    let myopenmls = MyOpenMls::new(
        key_material.clone(),
        group_commit_tx,
        instruction_tx.clone(),
        load_file_path,
        key_material.get_peer_id(),
    )?;

    let behaviour = MyBehaviour::new(&key_material.get_keypair())?;

    let mut abstswarm_builder = AbstSwarmBuilder::new(
        key_material.get_keypair(),
        instruction_rx,
        notification_tx,
        behaviour,
        myopenmls,
        group_commit_rx,
        storage,
    )?;

    if let Some(port) = args.port {
        abstswarm_builder =
            abstswarm_builder.listen_address(format!("/ip4/0.0.0.0/tcp/{port}").parse()?);
    } else {
        abstswarm_builder = abstswarm_builder.listen_address("/ip4/0.0.0.0/tcp/10001".parse()?);
    }

    if let Some(addr) = args.advertise_addr.clone() {
        if !addr.to_string().starts_with("/ip4/127.0.0.1/") {
            abstswarm_builder = abstswarm_builder.add_external_address(addr);
        }
    }

    let abstswarm = abstswarm_builder.build()?;

    let client = MyClient::new(instruction_tx, notification_rx);

    // Start network event loop
    let network = tokio::spawn(abstswarm.run());

    // Start client event loop
    let client = tokio::spawn(client.run());

    // Without its event loop the node only looks alive, e.g. after a panic in a handler:
    // exit non-zero so that Docker restarts it. exit() also ends a pending stdin read, which
    // returning from main would wait for.
    tokio::select! {
        result = network => {
            match result {
                Ok(()) => log::error!("Network event loop ended, exiting"),
                Err(e) => log::error!("Network event loop failed, exiting: {e}"),
            }
            exit(1);
        }
        result = client => result??,
    }

    Ok(())
}
