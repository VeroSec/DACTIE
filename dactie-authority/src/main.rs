use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use clap::{arg, Parser};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use dactie_utils::key_store::StoredKeyMaterial;
use crate::abstswarm::abstswarm_builder::AbstSwarmBuilder;
use crate::mybehaviour::MyBehaviour;
use dactie_utils::mls_wrapper::MyOpenMls;
use crate::myclient::MyClient;
use crate::storage::Storage;

mod abstswarm;
mod mybehaviour;
mod storage;
mod myclient;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Location of the folder, where the keymaterial should be saved
    #[arg(short= 'K', long)]
    km_dir: String,

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

    //Load Save-File
    let mut km_path = PathBuf::from_str(&args.km_dir).unwrap();
    km_path.push("key_material.json");
    let mut store_path = PathBuf::from_str(&args.km_dir).unwrap();
    store_path.push("storage.json");
    let (storage, key_material) = if km_path.exists() && store_path.exists(){
        let storage = Storage::from_file(store_path)?;
        let key_material = StoredKeyMaterial::from_file(km_path)?;
        (storage,key_material)
    } else {
        let (storage,key_material) = Storage::new(store_path);
        storage.to_file()?;
        key_material.to_file(km_path)?;
        (storage,key_material)
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
        key_material.get_peer_id())?;
    

    let behaviour = MyBehaviour::new(
        &key_material.get_keypair().into(),
    )?;


    let mut abstswarm_builder = AbstSwarmBuilder::new(key_material.get_keypair().into(), instruction_rx, notification_tx, behaviour,myopenmls, group_commit_rx, storage)?;


    if let Some(port) = args.port {
        abstswarm_builder =
            abstswarm_builder.listen_address(format!("/ip4/0.0.0.0/tcp/{port}").parse()?);
    } else {
            abstswarm_builder =
                abstswarm_builder.listen_address("/ip4/0.0.0.0/tcp/10001".parse()?);

    }

    let abstswarm = abstswarm_builder.build()?;

    let client = MyClient::new(instruction_tx, notification_rx);

    // Start network event loop
    tokio::spawn(abstswarm.run());

    // Start client event loop
    tokio::spawn(client.run()).await??;

    Ok(())
}