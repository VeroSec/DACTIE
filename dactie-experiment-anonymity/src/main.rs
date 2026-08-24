mod adversary;
mod experiment;
mod peer;

use crate::adversary::{clamp_coalition_size, coalition_size_for, run_attack, Estimator};
use crate::experiment::experiment_prop_peer_id;
use crate::peer::{MeshConfig, PubInterval};
use clap::Parser;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
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

    /// Gossipsub mesh degree D. The anonymity bound is stated over this, so it
    /// is configurable rather than fixed.
    #[clap(short = 'D', long, default_value_t = 12)]
    mesh_n: usize,

    /// Lower edge of the mesh band. Defaults to D-2.
    #[clap(long)]
    mesh_n_low: Option<usize>,

    /// Upper edge of the mesh band. Defaults to 2*D, which is what gossipsub is
    /// allowed to drift to between heartbeats. Pin this near D for runs that
    /// validate the bound, so the measured degree is the D the bound is over.
    #[clap(long)]
    mesh_n_high: Option<usize>,

    /// Fraction of non-mesh peers each peer gossips IHAVE to per heartbeat, or
    /// gossip_lazy peers, whichever is greater. libp2p defaults to 0.25.
    /// Setting this and --gossip-lazy to zero disables gossip emission, which
    /// separates direct exposure over the mesh from exposure via answering
    /// IWANT requests.
    #[clap(long, default_value_t = 0.25)]
    gossip_factor: f64,

    /// Floor on the number of peers gossiped to per heartbeat. libp2p
    /// defaults to 6.
    #[clap(long, default_value_t = 6)]
    gossip_lazy: usize,

    /// Colluding fractions f to evaluate the source-anonymity attack at. Each is
    /// converted to a coalition size of ceil(f * N), capped at the N-1 peers that
    /// are not the source.
    #[clap(long, value_delimiter = ',', default_values_t = [0.05f64, 0.1, 0.2, 0.5])]
    adversary_fractions: Vec<f64>,

    /// Absolute coalition sizes to evaluate, independent of N. The bound is
    /// stated over the coalition size, and c=1 is the single-attacker case the
    /// published equation describes - which no fraction can reach at large N.
    /// Merged with the sizes the fractions resolve to; duplicates run once.
    #[clap(long, value_delimiter = ',', default_values_t = [1usize])]
    coalition_sizes: Vec<usize>,

    /// Colluding subsets sampled per run per fraction. The attack is pure
    /// post-processing, so statistical power here is cheap.
    #[clap(long, default_value_t = 2000)]
    adversary_trials: usize,

    /// Seed for coalition sampling, so a reported figure can be reproduced.
    #[clap(long, default_value_t = 0)]
    adversary_seed: u64,

    /// Fraction of peers that must receive the measurement message for a run to
    /// count as valid. Every peer but the source should get it, so anything
    /// below 1.0 means the mesh did not carry the message across the network
    /// and the run measures propagation failure rather than anonymity.
    #[clap(long, default_value_t = 0.99)]
    min_delivery: f32,

    /// Abort the series after this many consecutive runs below --min-delivery.
    /// A sweep that has stopped propagating will not recover by moving to a
    /// larger network, so failing fast beats discovering it hours later. Set 0
    /// to never abort.
    #[clap(long, default_value_t = 3)]
    abort_after_low_delivery: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("Starting Experiment");
    let args = Args::parse();

    let mut x = args.out_path.clone();
    let mut avg = args.out_path.clone();
    // D belongs in the name. The adversary file already carries it, but these
    // two did not, so a sweep over D truncated each previous configuration's
    // per-run data on the next iteration and only the last D survived.
    x.push(format!(
        "experiment_{}_{}_{}_D{}_{:?}.csv",
        args.n_peers_min, args.n_peers_max, args.time, args.mesh_n, args.interval
    ));
    avg.push(format!(
        "experiment_avg_{}_{}_{}_D{}_{:?}.csv",
        args.n_peers_min, args.n_peers_max, args.time, args.mesh_n, args.interval
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

    // delivery_ratio is appended, so anything reading the first four fields
    // positionally keeps working.
    writeln!(
        &file,
        "total_peers,correct_receiver,other_peer,no_p_peers,delivery_ratio,latency_p50_us,latency_p95_us,latency_max_us"
    )?;
    writeln!(
        &file_avg,
        "total_peers,correct_receiver,other_peer,no_p_peers,delivery_ratio,latency_p50_us,latency_p95_us,latency_max_us"
    )?;

    let mut adv = args.out_path.clone();
    adv.push(format!(
        "adversary_{}_{}_{}_D{}_{:?}.csv",
        args.n_peers_min, args.n_peers_max, args.time, args.mesh_n, args.interval
    ));
    let adv_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(adv)?;
    writeln!(
        &adv_file,
        "total_peers,mesh_n,mesh_n_low,mesh_n_high,gossip_factor,gossip_lazy,run,coalition_size,f_effective,estimator,success_rate,delivery_ratio,direct_receipts"
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
        adv_file,
        mesh: MeshConfig::new(
            args.mesh_n,
            args.mesh_n_low,
            args.mesh_n_high,
            args.gossip_factor,
            args.gossip_lazy,
        ),
        adversary_fractions: args.adversary_fractions,
        coalition_sizes: args.coalition_sizes,
        adversary_trials: args.adversary_trials,
        adversary_seed: args.adversary_seed,
        min_delivery: args.min_delivery,
        abort_after_low_delivery: args.abort_after_low_delivery,
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
    adv_file: File,
    mesh: MeshConfig,
    adversary_fractions: Vec<f64>,
    coalition_sizes: Vec<usize>,
    adversary_trials: usize,
    adversary_seed: u64,
    min_delivery: f32,
    abort_after_low_delivery: usize,
}

async fn start_experiment_series(args: StartExperimentSeriesArgs) -> Result<(), Box<dyn Error>> {
    let mut consecutive_invalid = 0usize;
    let mut total_low_delivery = 0usize;
    let mut total_runs = 0usize;

    for n_peers in (args.min_n..=args.max_n).step_by(args.n_step_size) {
        let mut avg_correct_receiver = 0f32;
        let mut avg_other_peer = 0f32;
        let mut avg_no_p_peers = 0;
        let mut avg_delivery = 0f32;
        // The worst case is carried as a max rather than a mean: averaging
        // per-run maxima hides the tail this column exists to show.
        let mut avg_lat_p50 = 0u128;
        let mut avg_lat_p95 = 0u128;
        let mut worst_lat = 0u128;

        for i in 0..args.repetitions {
            let run = i + 1;
            let interval = args.interval.clone();
            println!(
                "Starting Run {run} from {repetitions}, with peer_n={n_peers}, time={time}secs, pub_interval={interval:?}",
                repetitions = args.repetitions,
                time = args.time,
            );
            let outcome = experiment_prop_peer_id(
                n_peers,
                Duration::from_secs(args.time),
                interval,
                args.mesh,
            )
            .await?;
            let (correct_receiver, other_peer, test) = (
                outcome.correct_receiver,
                outcome.other_peer,
                outcome.distinct_prop_peers,
            );

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

            let delivery = outcome.delivery_ratio();
            let (lat_p50, lat_p95, lat_max) = outcome.latency_us();
            total_runs += 1;
            println!(
                "Delivery: {:.1}% of {} peers",
                delivery * 100.0,
                n_peers - 1
            );
            println!("Latency: p50 {lat_p50} us, p95 {lat_p95} us, max {lat_max} us");
            if delivery < args.min_delivery {
                total_low_delivery += 1;
                eprintln!(
                    "WARNING: only {:.1}% of peers received the message (threshold {:.1}%). \
                     This run measures propagation failure, not anonymity: correct_receiver \
                     starts counting IWANT responses from the publisher rather than its mesh, \
                     and the attack samples colluders that hold no observation. Filter it out \
                     on delivery_ratio.",
                    delivery * 100.0,
                    args.min_delivery * 100.0
                );
            }

            // Complete delivery is necessary but not sufficient. The publisher
            // also answers IWANT requests, and those deliveries name it as
            // propagation_source just like a mesh link does. When the mesh is
            // too slow, gossip carries the message instead and direct receipts
            // climb well past the degree - observed at 2x to 7x mesh_n on runs
            // that delivered to 100% of peers. P(E_dir) = D/(|Nc|-1) assumes
            // direct receipt happens only over the mesh, so such a run exceeds
            // any bound derived from D no matter how healthy delivery looks.
            let direct_ok = correct_receiver <= args.mesh.high as f32;
            if !direct_ok {
                eprintln!(
                    "WARNING: {correct_receiver} peers received directly from the publisher, \
                     above the {} the mesh band permits. The publisher is serving IWANT \
                     requests, so this run overstates direct exposure and is not a valid \
                     test of a bound stated over D. Filter it out on direct_receipts.",
                    args.mesh.high
                );
            }

            if delivery < args.min_delivery || !direct_ok {
                consecutive_invalid += 1;
            } else {
                consecutive_invalid = 0;
            }

            writeln!(
                &args.file,
                "{n_peers},{correct_receiver},{other_peer},{test},{delivery},{lat_p50},{lat_p95},{lat_max}"
            )?;

            // Replay the recorded observations against colluding subsets. No
            // network re-run: the same trace is attacked at every size. A set
            // so that two fractions resolving to the same c do not run twice.
            let sizes: BTreeSet<usize> = args
                .adversary_fractions
                .iter()
                .map(|f| coalition_size_for(*f, n_peers))
                .chain(
                    args.coalition_sizes
                        .iter()
                        .map(|c| clamp_coalition_size(*c, n_peers)),
                )
                .collect();

            for c in sizes {
                // The stream must depend on the coalition size too, or every
                // size restarts the same one and the curves are not independent.
                let mut hasher = DefaultHasher::new();
                (args.adversary_seed, n_peers, run, c).hash(&mut hasher);
                let mut rng = StdRng::seed_from_u64(hasher.finish());

                let rates = run_attack(
                    &outcome.observations,
                    outcome.source,
                    &outcome.all_peers,
                    c,
                    args.adversary_trials,
                    &mut rng,
                );
                let f_effective = c as f64 / n_peers as f64;
                for estimator in Estimator::ALL {
                    let rate = rates[&estimator];
                    println!("  attack c={c} (f={f_effective:.4}) {estimator}: {rate:.4}");
                    writeln!(
                        &args.adv_file,
                        "{n_peers},{n},{low},{high},{gf},{gl},{run},{c},{f_effective},{estimator},{rate},{delivery},{correct_receiver}",
                        n = args.mesh.n,
                        gf = args.mesh.gossip_factor,
                        gl = args.mesh.gossip_lazy,
                        low = args.mesh.low,
                        high = args.mesh.high,
                    )?;
                }
            }

            avg_correct_receiver += correct_receiver;
            avg_other_peer += other_peer;
            avg_no_p_peers += test;
            avg_delivery += delivery;
            avg_lat_p50 += lat_p50;
            avg_lat_p95 += lat_p95;
            worst_lat = worst_lat.max(lat_max);

            if args.abort_after_low_delivery > 0
                && consecutive_invalid >= args.abort_after_low_delivery
            {
                return Err(format!(
                    "aborting: {consecutive_invalid} consecutive invalid runs (below the \
                     {:.1}% delivery threshold, or more direct receipts than the mesh band \
                     permits), most recently at n_peers={n_peers}. The mesh \
                     is not carrying the message across a network this large. Lower \
                     --n-peers-max, raise --time to give discovery longer, or widen the mesh \
                     band; a larger network will not recover on its own. Pass \
                     --abort-after-low-delivery 0 to continue anyway.",
                    args.min_delivery * 100.0
                )
                .into());
            }

            sleep(Duration::from_secs(10)).await
        }
        avg_correct_receiver /= args.repetitions as f32;
        avg_other_peer /= args.repetitions as f32;
        avg_no_p_peers /= args.repetitions;
        avg_delivery /= args.repetitions as f32;
        avg_lat_p50 /= args.repetitions as u128;
        avg_lat_p95 /= args.repetitions as u128;

        println!("All Repetitions done!");
        println!("Average messages direct from Publisher: {avg_correct_receiver}");
        println!("Average messages Relayed: {avg_other_peer}");
        println!(
            "Average Percentage of direct peers: {} %",
            avg_correct_receiver / (avg_correct_receiver + avg_other_peer) * 100f32
        );
        println!("Average Number of Propagating Peers: {avg_no_p_peers}");
        println!(
            "Average latency: p50 {avg_lat_p50} us, p95 {avg_lat_p95} us, worst {worst_lat} us"
        );

        writeln!(
            &args.avg_file,
            "{n_peers},{avg_correct_receiver},{avg_other_peer},{avg_no_p_peers},{avg_delivery},{avg_lat_p50},{avg_lat_p95},{worst_lat}"
        )?;
    }

    if total_low_delivery > 0 {
        eprintln!(
            "WARNING: {total_low_delivery} of {total_runs} runs fell below the delivery \
             threshold and are not valid anonymity measurements. Filter on delivery_ratio \
             before plotting."
        );
    }

    Ok(())
}
