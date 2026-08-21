use axum::{routing::get, Router};
use clap::Parser;
use libp2p::Multiaddr;
use libp2p_metrics::Metrics as Lp2pMetrics;
use prometheus_client::registry::Registry;
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use sysinfo::{CpuRefreshKind, System};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;

use myclient::MyClient;

use crate::abstswarm::abstswarm_builder::AbstSwarmBuilder;
use crate::mybehaviour::MyBehaviour;
use dactie_utils::key_store::StoredKeyMaterial;
use dactie_utils::mls_wrapper::MyOpenMls;
use libp2p::PeerId;

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
    #[arg(short = 'K', long)]
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

    /// Explicit external address to advertise (e.g. /ip4/10.244.192.119/tcp/21014)
    #[arg(long)]
    advertise_addr: Option<Multiaddr>,

    /// Serve Prometheus metrics on this address, e.g. 127.0.0.1:9898.
    /// Omitted by default: no registry is built and no socket is opened.
    /// The endpoint is unauthenticated, so bind it to loopback unless the
    /// network it is exposed on is trusted.
    #[arg(long)]
    metrics_addr: Option<SocketAddr>,
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

    // Ensure key material directory exists
    let km_dir = PathBuf::from_str(&args.km_dir)?;
    std::fs::create_dir_all(&km_dir)?;
    let mut km_path = km_dir.clone();
    km_path.push("key_material.json");

    //Get Group Key Material from Authorization Authority or File
    let key_material = if !km_path.exists() {
        let key_material = StoredKeyMaterial::new(None, None, None, None);
        key_material.to_file(km_path)?;
        key_material
    } else {
        StoredKeyMaterial::from_file(km_path)?
    };

    let (notification_tx, notification_rx) = mpsc::unbounded_channel();
    let (instruction_tx, instruction_rx) = mpsc::unbounded_channel();
    let (group_commit_tx, group_commit_rx) = mpsc::unbounded_channel();

    let load_file_path = PathBuf::from_str(&args.km_dir)?;

    let myopenmls = std::sync::Arc::new(MyOpenMls::new(
        key_material.clone(),
        group_commit_tx,
        instruction_tx.clone(),
        load_file_path,
        args.authority_peer_id.unwrap(),
    )?);

    let behaviour = MyBehaviour::new(
        &key_material.get_keypair(),
        args.authority_peer_id,
        args.bootstrap_addr,
    )?;

    let mut abstswarm_builder = AbstSwarmBuilder::new(
        key_material.get_keypair(),
        instruction_rx,
        notification_tx,
        behaviour,
        myopenmls.clone(),
        group_commit_rx,
    )?;

    if let Some(port) = args.port {
        abstswarm_builder =
            abstswarm_builder.listen_address(format!("/ip4/0.0.0.0/tcp/{port}").parse()?);
    } else if args.authority_peer_id.is_none() {
        abstswarm_builder = abstswarm_builder.listen_address("/ip4/0.0.0.0/tcp/10001".parse()?);
    }

    // Optionally advertise a host-reachable address so other peers dial the correct host:port
    if let Some(addr) = args.advertise_addr.clone() {
        if !addr.to_string().starts_with("/ip4/127.0.0.1/") {
            abstswarm_builder = abstswarm_builder.add_external_address(addr);
        }
    }

    // Prometheus metrics are opt-in via --metrics-addr. Without the flag no
    // registry is built, no recorder is attached to the swarm, and no socket
    // is opened.
    if let Some(metrics_addr) = args.metrics_addr {
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
                let mut reg = registry.lock().unwrap();
                // Use an f64 gauge to capture fractional CPU percentages
                let cpu_gauge: Gauge<f64, AtomicU64> = Gauge::default();
                let cpu_gauge_handle = cpu_gauge.clone();
                // Register without a unit (Unit::Percent does not exist in 0.22.2)
                reg.sub_registry_with_prefix("libp2p").register(
                    "cpu_process_percent",
                    "Approximate CPU utilization of this process (percent)",
                    cpu_gauge,
                );
                drop(reg);

                tokio::spawn({
                    let mut sys = System::new();
                    let cpu_gauge_handle = cpu_gauge_handle.clone();
                    async move {
                        loop {
                            sys.refresh_cpu_specifics(CpuRefreshKind::everything());
                            let global = sys.global_cpu_info();
                            let val = global.cpu_usage() as f64; // percent
                            cpu_gauge_handle.set(val);
                            // Sleep ~1s between samples
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
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
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(metrics_addr).await {
                Ok(listener) => {
                    log::info!(
                        "Metrics endpoint listening on http://{}/metrics",
                        metrics_addr
                    );
                    if let Err(e) = axum::serve(listener, metrics_app.into_make_service()).await {
                        log::error!("Metrics server stopped: {}", e);
                    }
                }
                // Previously an unwrap: a second peer sharing the host network
                // namespace lost the port and its metrics task died silently.
                Err(e) => log::error!("Could not bind metrics endpoint on {}: {}", metrics_addr, e),
            }
        });

        abstswarm_builder = abstswarm_builder.with_metrics(lp2p_metrics);
    }

    let abstswarm = abstswarm_builder.build()?;

    let client = MyClient::new(
        instruction_tx,
        notification_rx,
        key_material.get_peer_id(),
        myopenmls,
    );

    // Start network event loop
    tokio::spawn(abstswarm.run());

    // Start client event loop
    tokio::spawn(client.run()).await??;

    Ok(())
}
