use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use clap::{arg, Parser};
use libp2p::Multiaddr;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;

use myclient::MyClient;

use crate::abstswarm::abstswarm_builder::AbstSwarmBuilder;
use libp2p::PeerId;
use dactie_utils::key_store::StoredKeyMaterial;
use crate::mybehaviour::MyBehaviour;
use dactie_utils::mls_wrapper::MyOpenMls;

mod abstswarm;
mod mybehaviour;
mod myclient;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// PeerID of a Bootsrap Node, if none is given, the peer starts as Bootstrap Node on Port 10001
    #[arg(short='b', long, default_value = None)]
    authority_peer_id: Option<PeerId>,

    /// IPADDR of a Bootsrap Node, if none is given, the peer starts as Bootstrap Node on Port 10001
    #[arg(short='B', long, default_value_t = Multiaddr::from_str("/ip4/127.0.0.1/tcp/5005").unwrap())]
    bootstrap_addr: Multiaddr,


    /// Location of the folder, where the keymaterial should be saved
    #[arg(short= 'K', long)]
    km_dir: String,

    /// Number of Memberkeys to genereate
    #[arg(short, long, default_value_t = 1)]
    n_memkey: usize,

    /// Port to run service on
    #[arg(short, long)]
    port: Option<usize>,

    /// Load_file, for stored groups and subscribed topics
    #[arg(short, long)]
    load_file: Option<String>,

}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    // Handle Sigterm for Docker
    tokio::spawn(async move {
        let mut term_signal = signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal handler");
        term_signal.recv().await;
        println!("Received SIGTERM, shutting down gracefully...");
        exit(0);
    });

    let args = Args::parse();

    let mut km_path = PathBuf::from_str(&args.km_dir)?;
    km_path.push("key_material.json");

    //Get Group Key Material from Authorization Authority or File
    let key_material = if !km_path.exists(){
        let key_material = StoredKeyMaterial::new(None,None,None,None);
        key_material.to_file(km_path)?;
        key_material
    } else {
        StoredKeyMaterial::from_file(km_path)?
    };


    let (notification_tx, notification_rx) = mpsc::unbounded_channel();
    let (instruction_tx, instruction_rx) = mpsc::unbounded_channel();
    let (group_commit_tx, group_commit_rx) = mpsc::unbounded_channel();

    let load_file_path = PathBuf::from_str(&args.km_dir)?;

    let myopenmls = MyOpenMls::new(
        key_material.clone(),
        group_commit_tx,
        instruction_tx.clone(),
        load_file_path,
        args.authority_peer_id.unwrap())?;


    let behaviour = MyBehaviour::new(
        &key_material.get_keypair().into(),
        args.authority_peer_id,
        args.bootstrap_addr,
    )?;


    let mut abstswarm_builder = AbstSwarmBuilder::new(key_material.get_keypair().into(), instruction_rx, notification_tx, behaviour,myopenmls, group_commit_rx)?;


    if let Some(port) = args.port {
        abstswarm_builder =
            abstswarm_builder.listen_address(format!("/ip4/0.0.0.0/tcp/{port}").parse()?);
    } else if args.authority_peer_id.is_none() {
            abstswarm_builder =
                abstswarm_builder.listen_address("/ip4/0.0.0.0/tcp/10001".parse()?);

    }

    let abstswarm = abstswarm_builder.build()?;

    let client = MyClient::new(instruction_tx, notification_rx, key_material.get_peer_id());

    // Start network event loop
    tokio::spawn(abstswarm.run());

    // Start client event loop
    tokio::spawn(client.run()).await??;

    Ok(())
}