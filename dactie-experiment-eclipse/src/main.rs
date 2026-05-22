mod experiment;
mod bad_peer;
mod peer;
mod shared_structs;

use std::error::Error;
use std::fs::{File, OpenOptions};
use std::path::{PathBuf};
use std::time::Duration;
use tokio::time::{sleep};
use crate::experiment::experiment_prop_peer_id;
use clap::{arg, Parser};
use std::io::Write;
use shared_structs::*;

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
    #[arg(short= 'A', long, default_value_t = 25)]
    attack_n: usize,


    /// How often Peers Should publish. Input can be fix=<secs> or random=>start,end>
    #[clap(short, long, default_value="fix=5")]
    interval: PubInterval,

    /// How long before measurement message is send
    #[clap(short, long, default_value_t=10)]
    stop_time: u64,

    /// How long before attackers start to attack
    #[clap(short, long, default_value_t=10)]
    attack_time: u64,

    /// How often the experiment should be repeated
    #[clap(short, long, default_value_t=5)]
    repetitions: usize,

    /// Folder where the result should be saved
    #[clap(short, long, default_value="/tmp/")]
    out_path: PathBuf,




}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("Starting Experiment");
    let args = Args::parse();

    let mut x = args.out_path.clone();
    let mut avg = args.out_path.clone();
    x.push(format!("experiment_{}_{}_{}_{:?}.csv", args.n_peers_min,args.n_peers_max,args.stop_time,args.interval));
    avg.push(format!("experiment_avg_{}_{}_{}_{:?}.csv", args.n_peers_min,args.n_peers_max,args.stop_time,args.interval));

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(x)?;

    let file_avg = OpenOptions::new()
        .write(true)
        .create(true)
        .open(avg)?;

    writeln!(&file, "total_peers,honest_peers,attacker_peers,received_messages")?;
    writeln!(&file_avg, "total_peers,honest_peers,attacker_peers,received_messages")?;

    start_experiment_series(args.n_peers_min, args.n_peers_max, args.n_peers_step_size, args.attack_n, args.repetitions, args.attack_time,args.stop_time, args.interval, file, file_avg).await?;



    Ok(())
}

/**
Start an experiment series with the specified parameters.
The output is stored to the specified files
**/
async fn start_experiment_series(min_n: usize, max_n: usize, n_step_size:usize, attack_n: usize, repetitions: usize, attack_time: u64, stop_time: u64, interval: PubInterval, file: File,avg_file: File) -> Result<(), Box<dyn Error>> {
    for n_peers in (min_n..=max_n).step_by(n_step_size) {
        let mut avg_receivers = 0f32;


        for i in 0..repetitions {
            println!("Starting Run {} from {}, with peer_n={}, attack_time={}secs, pub_interval={:?}", i+1, repetitions, n_peers, attack_time, interval.clone());
            let receivers = experiment_prop_peer_id(n_peers, attack_n,Duration::from_secs(attack_time),Duration::from_secs(stop_time), interval.clone()).await?;


            println!("{} Peers from {} received victim messages.", receivers, n_peers);

            writeln!(&file, "{},{},{},{}", n_peers+attack_n, n_peers, attack_n, receivers)?;

            avg_receivers += receivers;

            sleep(Duration::from_secs(10)).await
        }
        avg_receivers = avg_receivers / repetitions as f32;

        println!("All Repetitions done!");
        println!("On Average {} Peers from {} received victim messages.", avg_receivers, n_peers);

        writeln!(&avg_file, "{},{},{},{}", n_peers+attack_n, n_peers, attack_n, avg_receivers)?;
    }

    Ok(())
}

