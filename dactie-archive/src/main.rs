use crate::abstswarm::abstswarm_builder::AbstSwarmBuilder;
use crate::mybehaviour::MyBehaviour;
use clap::Parser;
use dactie_utils::key_store::StoredKeyMaterial;
use dactie_utils::mls_wrapper::MyOpenMls;
use dactie_utils::shared_structs::Instruction;
use libp2p::{Multiaddr, PeerId};
use sqlx::postgres::PgPoolOptions;
use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;

mod abstswarm;
mod mybehaviour;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Location of the folder, where the keymaterial should be saved
    #[arg(short = 'K', long)]
    km_dir: String,

    /// Port to run service on
    #[arg(short, long)]
    port: Option<usize>,

    /// Port to run service on
    #[arg(short, long)]
    url: String,

    /// Explicit external address to advertise (e.g. /ip4/10.244.192.117/tcp/20011)
    #[arg(long)]
    advertise_addr: Option<Multiaddr>,

    /// Authorization Authority ID
    #[arg(short, long)]
    aa_id: PeerId,

    /// Address of a Authorization Node
    #[arg(short='B', long, default_value_t = Multiaddr::from_str("/ip4/127.0.0.1/tcp/20011").unwrap())]
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
        let mut term_signal =
            signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal handler");
        term_signal.recv().await;
        println!("Received SIGTERM, shutting down gracefully...");
        exit(0);
    });

    let mut km_path = PathBuf::from_str(&args.km_dir)?;
    km_path.push("key_material.json");

    let (instruction_tx, instruction_rx) = mpsc::unbounded_channel();
    let (group_commit_tx, _) = mpsc::unbounded_channel();

    //Get Group Key Material from Authorization Authority or File
    //Registration is driven by the group key, not by the file: the file is written before
    //the authority answers, so its mere presence does not mean registration succeeded.
    let register = |identity: String| -> Result<(), Box<dyn Error>> {
        log::info!(
            "Registering identity '{}' with authority {}",
            identity,
            args.aa_id
        );
        instruction_tx.send(Instruction::RegisterIdentity {
            peer_id: args.aa_id,
            identity,
        })?;
        Ok(())
    };
    let key_material = if km_path.exists() {
        let key_material = StoredKeyMaterial::from_file(km_path.clone())?;
        if key_material.get_grpkey().is_none() {
            log::warn!("Key material has no group key, re-registering with the authority");
            register(args.identity.clone())?;
        } else {
            log::info!("Group key present, skipping registration");
        }
        key_material
    } else {
        register(args.identity.clone())?;
        let key_material = StoredKeyMaterial::new(None, None, None, None);
        key_material.to_file(km_path)?;
        key_material
    };

    // Connect to the database
    log::info!("Connecting to Postgres at {}", args.url);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&args.url)
        .await?;
    log::info!("Postgres connection established");

    let load_file_path = PathBuf::from_str(&args.km_dir)?;

    let myopenmls = MyOpenMls::new(
        key_material.clone(),
        group_commit_tx,
        instruction_tx.clone(),
        load_file_path,
        args.aa_id,
    )?;

    let behaviour = MyBehaviour::new(&key_material.get_keypair(), args.aa_id, args.aa_addr)?;

    let mut abstswarm_builder = AbstSwarmBuilder::new(
        key_material.get_keypair(),
        instruction_rx,
        pool,
        behaviour,
        myopenmls,
    )?;

    if let Some(port) = args.port {
        abstswarm_builder =
            abstswarm_builder.listen_address(format!("/ip4/0.0.0.0/tcp/{port}").parse()?);
    }

    if let Some(addr) = args.advertise_addr.clone() {
        if !addr.to_string().starts_with("/ip4/127.0.0.1/") {
            abstswarm_builder = abstswarm_builder.add_external_address(addr);
        }
    }

    let abstswarm = abstswarm_builder.build()?;

    // Start network event loop
    tokio::spawn(abstswarm.run()).await?;

    Ok(())
}
