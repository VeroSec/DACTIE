//! Where a series writes: its log, and one row per run appended to a results CSV.

use crate::shared_structs::{GossipConfig, PubInterval};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Opens the log that `say!` appends to. A series run again appends to the same log.
pub(crate) fn open_log(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    LOG.set(Mutex::new(file))
        .map_err(|_| io::Error::other("log already open"))
}

/// Prints a line and appends it to the log, if one is open.
pub(crate) fn log_line(line: &str) {
    println!("{line}");
    if let Some(Ok(mut file)) = LOG.get().map(Mutex::lock) {
        // A failing log must not end a run that takes minutes; stdout still has the line.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

/// `println!` that also goes to the series' log.
macro_rules! say {
    ($($arg:tt)*) => { $crate::output::log_line(&format!($($arg)*)) };
}

pub(crate) const RESULTS_HEADER: &str = "total_peers,honest_peers,attacker_peers,attacker_ratio,\
mesh_n_low,mesh_n,mesh_n_high,flood_publish,gossip_lazy,gossip_factor,attack_time,stop_time,\
interval,run,received_messages,message_ratio";

/// One run: the victim's message reached `received` of the `honest - 1` other honest peers.
pub(crate) struct RunResult {
    pub(crate) honest: usize,
    pub(crate) attackers: usize,
    pub(crate) gossip: GossipConfig,
    pub(crate) attack_time: u64,
    pub(crate) stop_time: u64,
    pub(crate) interval: PubInterval,
    pub(crate) run: usize,
    pub(crate) received: usize,
}

impl RunResult {
    fn csv_line(&self) -> String {
        let total = self.honest + self.attackers;
        let attacker_ratio = self.attackers as f64 / total as f64;
        let message_ratio = self.received as f64 / self.honest.saturating_sub(1).max(1) as f64;
        let g = &self.gossip;
        format!(
            "{total},{honest},{attackers},{attacker_ratio},{low},{n},{high},{flood},{lazy},{factor},\
             {attack_time},{stop_time},{interval},{run},{received},{message_ratio}\n",
            honest = self.honest,
            attackers = self.attackers,
            low = g.mesh.low,
            n = g.mesh.n,
            high = g.mesh.high,
            flood = g.flood_publish,
            lazy = g.gossip_lazy,
            factor = g.gossip_factor,
            attack_time = self.attack_time,
            stop_time = self.stop_time,
            interval = self.interval,
            run = self.run,
            received = self.received,
        )
    }
}

/// A results CSV that every run appends one row to. Several series, also in parallel
/// processes, can share one file: each row is a single write to a file opened for appending.
pub(crate) struct Results {
    file: File,
}

impl Results {
    /// Creates the file with its header, or appends to it if its header is the current one.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                file.write_all(format!("{RESULTS_HEADER}\n").as_bytes())?;
                return Ok(Self { file });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let mut first = String::new();
        BufReader::new(File::open(path)?).read_line(&mut first)?;
        let mut file = OpenOptions::new().append(true).open(path)?;
        if first.is_empty() {
            file.write_all(format!("{RESULTS_HEADER}\n").as_bytes())?;
        } else if first.trim_end() != RESULTS_HEADER {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "{} has other columns than this version writes; give --results a new file",
                    path.display()
                ),
            ));
        }
        Ok(Self { file })
    }

    pub(crate) fn append(&mut self, result: &RunResult) -> io::Result<()> {
        self.file.write_all(result.csv_line().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_structs::MeshConfig;

    fn result(received: usize) -> RunResult {
        RunResult {
            honest: 3,
            attackers: 9,
            gossip: GossipConfig {
                flood_publish: false,
                mesh: MeshConfig::new(12, None, None),
                gossip_lazy: 6,
                gossip_factor: 0.25,
                bootstrap_port: 10001,
            },
            attack_time: 10,
            stop_time: 180,
            interval: PubInterval::Random((3, 10)),
            run: 2,
            received,
        }
    }

    #[test]
    fn a_row_matches_the_header() {
        let line = result(1).csv_line();
        assert_eq!(
            line,
            "12,3,9,0.75,10,12,24,false,6,0.25,10,180,random=3-10,2,1,0.5\n"
        );
        assert_eq!(
            line.trim_end().split(',').count(),
            RESULTS_HEADER.split(',').count()
        );
    }

    #[test]
    fn rows_append_under_one_header() {
        let dir = std::env::temp_dir().join(format!("eclipse-results-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("results.csv");
        let _ = std::fs::remove_file(&path);
        Results::open(&path).unwrap().append(&result(0)).unwrap();
        Results::open(&path).unwrap().append(&result(2)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], RESULTS_HEADER);
        assert!(lines[1].ends_with(",0,0"));
        assert!(lines[2].ends_with(",2,1"));

        std::fs::write(&path, "other,columns\n").unwrap();
        assert!(Results::open(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
