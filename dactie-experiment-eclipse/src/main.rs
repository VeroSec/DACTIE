mod bad_peer;
mod experiment;
mod peer;
mod shared_structs;

use crate::experiment::experiment_prop_peer_id;
use clap::{arg, Parser};
use shared_structs::*;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Min number of peers in series
    #[arg(short, long, default_value_t = 2)]
    n_peers_min: usize,

    /// Max number of peers in series
    #[arg(short = 'N', long, default_value_t = 20)]
    n_peers_max: usize,

    /// Step Size of Series
    #[arg(short = 'S', long, default_value_t = 1)]
    n_peers_step_size: usize,

    /// Step Size of Series
    #[arg(short = 'A', long, default_value_t = 25)]
    attack_n: usize,

    /// How often Peers Should publish. Input can be fix=<secs> or random=>start,end>
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

    /// Folder where the result should be saved
    #[clap(short, long, default_value = "/tmp/")]
    out_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("Starting Experiment");
    let args = Args::parse();

    let mut x = args.out_path.clone();
    let mut avg = args.out_path.clone();
    x.push(format!(
        "experiment_{}_{}_{}_{:?}.csv",
        args.n_peers_min, args.n_peers_max, args.stop_time, args.interval
    ));
    avg.push(format!(
        "experiment_avg_{}_{}_{}_{:?}.csv",
        args.n_peers_min, args.n_peers_max, args.stop_time, args.interval
    ));

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(x)?;

    let file_avg = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(avg)?;

    writeln!(
        &file,
        "total_peers,honest_peers,attacker_peers,received_messages"
    )?;
    writeln!(
        &file_avg,
        "total_peers,honest_peers,attacker_peers,received_messages"
    )?;

    start_experiment_series(StartExperimentSeriesArgs {
        min_n: args.n_peers_min,
        max_n: args.n_peers_max,
        n_step_size: args.n_peers_step_size,
        attack_n: args.attack_n,
        repetitions: args.repetitions,
        attack_time: args.attack_time,
        stop_time: args.stop_time,
        interval: args.interval,
        file,
        avg_file: file_avg,
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
    file: File,
    avg_file: File,
}

async fn start_experiment_series(args: StartExperimentSeriesArgs) -> Result<(), Box<dyn Error>> {
    for n_peers in (args.min_n..=args.max_n).step_by(args.n_step_size) {
        let mut avg_receivers = 0f32;

        for i in 0..args.repetitions {
            let run = i + 1;
            let interval = args.interval.clone();
            println!(
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
            )
            .await?;

            println!("{receivers} Peers from {n_peers} received victim messages.");

            let total_peers = n_peers + args.attack_n;
            writeln!(
                &args.file,
                "{total_peers},{n_peers},{attack_n},{receivers}",
                attack_n = args.attack_n
            )?;

            avg_receivers += receivers;

            sleep(Duration::from_secs(10)).await
        }
        avg_receivers /= args.repetitions as f32;

        println!("All Repetitions done!");
        println!("On Average {avg_receivers} Peers from {n_peers} received victim messages.");

        let total_peers = n_peers + args.attack_n;
        writeln!(
            &args.avg_file,
            "{total_peers},{n_peers},{attack_n},{avg_receivers}",
            attack_n = args.attack_n
        )?;
    }

    Ok(())
}
