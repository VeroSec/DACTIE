use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use clap::{arg, Parser};
use libp2p::{Multiaddr, PeerId};
use sqlx::postgres::PgPoolOptions;
use crate::abstswarm::abstswarm_builder::AbstSwarmBuilder;
use crate::mybehaviour::MyBehaviour;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use dactie_utils::key_store::StoredKeyMaterial;
use dactie_utils::mls_wrapper::MyOpenMls;
use dactie_utils::shared_structs::Instruction;

mod abstswarm;
mod mybehaviour;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Location of the folder, where the keymaterial should be saved
    #[arg(short= 'K', long)]
    km_dir: String,

    /// Port to run service on
    #[arg(short, long)]
    port: Option<usize>,

    /// Port to run service on
    #[arg(short, long)]
    url: String,

    /// Authorization Authority ID
    #[arg(short, long)]
    aa_id: PeerId,

    /// Address of a Authorization Node
    #[arg(short='B', long, default_value_t = Multiaddr::from_str("/ip4/127.0.0.1/tcp/5005").unwrap())]
    aa_addr: Multiaddr,

    /// Identity of Archive
    #[arg(short, long)]
    identity: String,

}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let args = Args::parse();

    // Handle Sigterm for Docker
    tokio::spawn(async move {
        let mut term_signal = signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal handler");
        term_signal.recv().await;
        println!("Received SIGTERM, shutting down gracefully...");
        exit(0);
    });

    let mut km_path = PathBuf::from_str(&args.km_dir)?;
    km_path.push("key_material.json");

    let (instruction_tx, instruction_rx) = mpsc::unbounded_channel();
    let (group_commit_tx, _) = mpsc::unbounded_channel();

    //Get Group Key Material from Authorization Authority or File
    let key_material = if !km_path.exists(){
        let instruction = Instruction::RegisterIdentity {peer_id: args.aa_id, identity: args.identity};
        instruction_tx.send(instruction)?;
        let key_material = StoredKeyMaterial::new(None, None, None,None);
        key_material.to_file(km_path)?;
        key_material
    } else {
        StoredKeyMaterial::from_file(km_path)?
    };

    // Connect to the database
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&args.url)
        .await?;



    let load_file_path = PathBuf::from_str(&args.km_dir)?;

    let myopenmls = MyOpenMls::new(
        key_material.clone(),
        group_commit_tx,
        instruction_tx.clone(),
        load_file_path,
        args.aa_id)?;

    let behaviour = MyBehaviour::new(
        &key_material.get_keypair().into(),
        args.aa_id,
        args.aa_addr
    )?;


    let mut abstswarm_builder = AbstSwarmBuilder::new(key_material.get_keypair().into(),instruction_rx,pool,behaviour, myopenmls)?;


    if let Some(port) = args.port {
        abstswarm_builder =
            abstswarm_builder.listen_address(format!("/ip4/0.0.0.0/tcp/{port}").parse()?);
    }

    let abstswarm = abstswarm_builder.build()?;


    // Start network event loop
    tokio::spawn(abstswarm.run()).await?;


    Ok(())
}