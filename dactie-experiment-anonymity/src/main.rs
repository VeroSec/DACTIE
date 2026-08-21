mod experiment;
mod peer;

use crate::experiment::experiment_prop_peer_id;
use crate::peer::PubInterval;
use clap::Parser;
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

    /// How often Peers Should publish. Input can be fix=<secs> or random=>start,end>
    #[clap(short, long, default_value = "fix=5")]
    interval: PubInterval,

    /// How long before measurement message is send
    #[clap(short, long, default_value_t = 10)]
    time: u64,

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
        args.n_peers_min, args.n_peers_max, args.time, args.interval
    ));
    avg.push(format!(
        "experiment_avg_{}_{}_{}_{:?}.csv",
        args.n_peers_min, args.n_peers_max, args.time, args.interval
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

    writeln!(&file, "total_peers,correct_receiver,other_peer,no_p_peers")?;
    writeln!(
        &file_avg,
        "total_peers,correct_receiver,other_peer,no_p_peers"
    )?;

    start_experiment_series(StartExperimentSeriesArgs {
        min_n: args.n_peers_min,
        max_n: args.n_peers_max,
        n_step_size: args.n_peers_step_size,
        repetitions: args.repetitions,
        time: args.time,
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
    repetitions: usize,
    time: u64,
    interval: PubInterval,
    file: File,
    avg_file: File,
}

async fn start_experiment_series(args: StartExperimentSeriesArgs) -> Result<(), Box<dyn Error>> {
    for n_peers in (args.min_n..=args.max_n).step_by(args.n_step_size) {
        let mut avg_correct_receiver = 0f32;
        let mut avg_other_peer = 0f32;
        let mut avg_no_p_peers = 0;

        for i in 0..args.repetitions {
            let run = i + 1;
            let interval = args.interval.clone();
            println!(
                "Starting Run {run} from {repetitions}, with peer_n={n_peers}, time={time}secs, pub_interval={interval:?}",
                repetitions = args.repetitions,
                time = args.time,
            );
            let (correct_receiver, other_peer, test) =
                experiment_prop_peer_id(n_peers, Duration::from_secs(args.time), interval).await?;

            println!(
                "Received {} messages from {} peers.",
                correct_receiver + other_peer,
                n_peers - 1
            );
            println!("Direct from Publisher: {correct_receiver}");
            println!("Relayed: {other_peer}");
            println!(
                "Percentage of direct peers: {} %",
                correct_receiver / (correct_receiver + other_peer) * 100f32
            );
            println!("Number of Propagating Peers: {test}");

            writeln!(
                &args.file,
                "{n_peers},{correct_receiver},{other_peer},{test}"
            )?;

            avg_correct_receiver += correct_receiver;
            avg_other_peer += other_peer;
            avg_no_p_peers += test;

            sleep(Duration::from_secs(10)).await
        }
        avg_correct_receiver /= args.repetitions as f32;
        avg_other_peer /= args.repetitions as f32;
        avg_no_p_peers /= args.repetitions;

        println!("All Repetitions done!");
        println!("Average messages direct from Publisher: {avg_correct_receiver}");
        println!("Average messages Relayed: {avg_other_peer}");
        println!(
            "Average Percentage of direct peers: {} %",
            avg_correct_receiver / (avg_correct_receiver + avg_other_peer) * 100f32
        );
        println!("Average Number of Propagating Peers: {avg_no_p_peers}");

        writeln!(
            &args.avg_file,
            "{n_peers},{avg_correct_receiver},{avg_other_peer},{avg_no_p_peers}"
        )?;
    }

    Ok(())
}
