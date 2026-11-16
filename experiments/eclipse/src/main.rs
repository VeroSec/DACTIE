#[macro_use]
mod output;
mod bad_peer;
mod experiment;
mod peer;
mod shared_structs;

use crate::experiment::experiment_prop_peer_id;
use clap::Parser;
use output::{Results, RunResult};
use shared_structs::*;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Smallest number of honest peers in the series, including the victim
    #[arg(short, long, default_value_t = 2)]
    n_peers_min: usize,

    /// Largest number of honest peers in the series
    #[arg(short = 'N', long, default_value_t = 20)]
    n_peers_max: usize,

    /// Step between the numbers of honest peers
    #[arg(short = 'S', long, default_value_t = 1)]
    n_peers_step_size: usize,

    /// Number of attacker peers
    #[arg(short = 'A', long, default_value_t = 25)]
    attack_n: usize,

    /// How often peers publish: fix=<secs> or random=<start>,<end>
    #[clap(short, long, default_value = "fix=5")]
    interval: PubInterval,

    /// How long before measurement message is send
    #[clap(short, long, default_value_t = 10)]
    stop_time: u64,

    /// How long before attackers start to attack
    #[clap(short, long, default_value_t = 10)]
    attack_time: u64,

    /// How often the experiment should be repeated
    #[clap(short, long, default_value_t = 5)]
    repetitions: usize,

    /// Folder for the series' log, eclipse_<series>.log, which a series run again appends
    /// to. It stays local; the results go to --results
    #[clap(short, long, default_value = "/tmp/")]
    out_path: PathBuf,

    /// CSV file that every run appends one row to, created with a header if missing.
    /// Several series, also running in parallel, can share it [default: <out-path>/eclipse_results.csv]
    #[clap(long)]
    results: Option<PathBuf>,

    /// Whether the victim floods the message to every subscriber instead of
    /// only its mesh. This is the trade-off under test: flood publishing
    /// defeats an eclipse but leaks the sender to every peer at once.
    #[clap(long, action = clap::ArgAction::Set, default_value_t = false)]
    flood_publish: bool,

    /// Gossipsub mesh degree D, matching the anonymity experiment so the two
    /// halves of the trade-off are measured at the same configuration.
    #[clap(short = 'D', long, default_value_t = 12)]
    mesh_n: usize,

    /// Lower edge of the mesh band. Defaults to D-2.
    #[clap(long)]
    mesh_n_low: Option<usize>,

    /// Upper edge of the mesh band. Defaults to 2*D.
    #[clap(long)]
    mesh_n_high: Option<usize>,

    /// Gossip emission: peers announced per heartbeat at least (libp2p's default, as in the
    /// DACTIE nodes). With --gossip-factor 0 as well, emission is off.
    #[clap(long, default_value_t = 6)]
    gossip_lazy: usize,

    /// Gossip emission: share of non-mesh peers announced per heartbeat (libp2p's default,
    /// as in the DACTIE nodes).
    #[clap(long, default_value_t = 0.25)]
    gossip_factor: f64,

    /// TCP port the bootstrap peer binds. Give consecutive invocations
    /// different ports: the previous run's peers may still hold the socket,
    /// and the bind fails with AddrInUse before a single run has started.
    #[clap(long, default_value_t = 10001)]
    bootstrap_port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // The settings go in the log's name, so that series running in parallel in one folder
    // keep separate logs.
    let series = format!(
        "eclipse_{}_{}_{}_A{}_D{}_flood{}_lazy{}_factor{}_{}",
        args.n_peers_min,
        args.n_peers_max,
        args.stop_time,
        args.attack_n,
        args.mesh_n,
        args.flood_publish,
        args.gossip_lazy,
        args.gossip_factor,
        args.interval
    );
    std::fs::create_dir_all(&args.out_path)?;
    output::open_log(&args.out_path.join(format!("{series}.log")))?;
    let results_path = args
        .results
        .clone()
        .unwrap_or_else(|| args.out_path.join("eclipse_results.csv"));
    let results = Results::open(&results_path)?;
    say!(
        "Starting Experiment {series}, appending results to {}",
        results_path.display()
    );

    start_experiment_series(StartExperimentSeriesArgs {
        min_n: args.n_peers_min,
        max_n: args.n_peers_max,
        n_step_size: args.n_peers_step_size,
        attack_n: args.attack_n,
        repetitions: args.repetitions,
        attack_time: args.attack_time,
        stop_time: args.stop_time,
        interval: args.interval,
        results,
        gossip: GossipConfig {
            flood_publish: args.flood_publish,
            mesh: MeshConfig::new(args.mesh_n, args.mesh_n_low, args.mesh_n_high),
            gossip_lazy: args.gossip_lazy,
            gossip_factor: args.gossip_factor,
            bootstrap_port: args.bootstrap_port,
        },
    })
    .await?;

    Ok(())
}

/**
Start an experiment series with the specified parameters.
The output is stored to the specified files
**/
struct StartExperimentSeriesArgs {
    min_n: usize,
    max_n: usize,
    n_step_size: usize,
    attack_n: usize,
    repetitions: usize,
    attack_time: u64,
    stop_time: u64,
    interval: PubInterval,
    results: Results,
    gossip: GossipConfig,
}

async fn start_experiment_series(
    mut args: StartExperimentSeriesArgs,
) -> Result<(), Box<dyn Error>> {
    for n_peers in (args.min_n..=args.max_n).step_by(args.n_step_size) {
        let mut avg_receivers = 0f32;

        for i in 0..args.repetitions {
            let run = i + 1;
            let interval = args.interval.clone();
            say!(
                "Starting Run {run} from {repetitions}, with peer_n={n_peers}, attack_time={attack_time}secs, pub_interval={interval:?}",
                repetitions = args.repetitions,
                attack_time = args.attack_time,
            );
            let receivers = experiment_prop_peer_id(
                n_peers,
                args.attack_n,
                Duration::from_secs(args.attack_time),
                Duration::from_secs(args.stop_time),
                interval,
                args.gossip,
            )
            .await?;

            say!("{receivers} Peers from {n_peers} received victim messages.");

            // Written as soon as the run ends, so that an interrupted series keeps its runs.
            args.results.append(&RunResult {
                honest: n_peers,
                attackers: args.attack_n,
                gossip: args.gossip,
                attack_time: args.attack_time,
                stop_time: args.stop_time,
                interval: args.interval.clone(),
                run,
                received: receivers as usize,
            })?;

            avg_receivers += receivers;

            sleep(Duration::from_secs(10)).await
        }
        avg_receivers /= args.repetitions as f32;

        say!("All Repetitions done!");
        say!("On Average {avg_receivers} Peers from {n_peers} received victim messages.");
    }

    Ok(())
}
