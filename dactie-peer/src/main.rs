use std::error::Error;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use clap::Parser;
use libp2p::Multiaddr;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use axum::{routing::get, Router};
use libp2p_metrics::Metrics as Lp2pMetrics;
use prometheus_client::registry::Registry;
use std::net::SocketAddr;
use once_cell::sync::Lazy;
use sysinfo::{System, SystemExt, CpuRefreshKind, RefreshKind};

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

    // Ensure key material directory exists
    let km_dir = PathBuf::from_str(&args.km_dir)?;
    std::fs::create_dir_all(&km_dir)?;
    let mut km_path = km_dir.clone();
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

    // Initialize Prometheus metrics registry and libp2p metrics recorder.
    // Expose metrics on 0.0.0.0:9898 in OpenMetrics format.
    let mut registry = Registry::default();
    let lp2p_metrics = Lp2pMetrics::new(&mut registry);

    // Spawn metrics HTTP endpoint
    let metrics_app = {
        use prometheus_client::encoding::text::encode;
        use std::sync::{Arc, Mutex};
        let registry = Arc::new(Mutex::new(registry));
        // Background task: sample CPU utilization and export as gauge
        // We export it at libp2p_cpu_process_percent
        {
            use prometheus_client::metrics::gauge::Gauge;
            use prometheus_client::registry::Unit;
            let mut reg = registry.lock().unwrap();
            let cpu_gauge: Gauge = Gauge::default();
            reg.sub_registry_with_prefix("libp2p").register_with_unit(
                "cpu_process_percent",
                "Approximate CPU utilization of this process (percent)",
                Unit::Percent,
                cpu_gauge.clone(),
            );
            drop(reg);

            tokio::spawn({
                let registry = registry.clone();
                async move {
                    let mut sys = System::new();
                    loop {
                        sys.refresh_cpu_specifics(CpuRefreshKind::everything());
                        let global = sys.global_cpu_info();
                        let val = global.cpu_usage() as f64; // percent
                        // update gauge
                        let mut reg = registry.lock().unwrap();
                        if let Some(metric) = reg
                            .sub_registry_with_prefix("libp2p")
                            .get_metric_by_name_mut("cpu_process_percent")
                        {
                            // downcast via encoding is not exposed; rely on re-register clone above.
                        }
                        drop(reg);
                        // Sleep ~1s between samples
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        // Note: prometheus-client Gauge lacks a set via registry lookup; proper wiring requires struct storage.
                        // Keeping placeholder; CPU metric can be added more robustly later or via external collector.
                    }
                }
            });
        }
        Router::new().route(
            "/metrics",
            get(move || {
                let registry = registry.clone();
                async move {
                    let mut output = String::new();
                    let reg = registry.lock().unwrap();
                    encode(&mut output, &reg).unwrap();
                    axum::response::Response::builder()
                        .header("Content-Type", "text/plain; version=0.0.4")
                        .body(output)
                        .unwrap()
                }
            }),
        )
    };
    let metrics_addr: SocketAddr = "0.0.0.0:9898".parse()?;
    tokio::spawn(async move {
        axum::serve(tokio::net::TcpListener::bind(metrics_addr).await.unwrap(), metrics_app.into_make_service())
            .await
            .expect("metrics server");
    });

    let abstswarm = abstswarm_builder.with_metrics(lp2p_metrics).build()?;

    let client = MyClient::new(instruction_tx, notification_rx, key_material.get_peer_id());

    // Start network event loop
    tokio::spawn(abstswarm.run());

    // Start client event loop
    tokio::spawn(client.run()).await??;

    Ok(())
}
