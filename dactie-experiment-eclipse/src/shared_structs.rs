use libp2p::PeerId;
use std::str::FromStr;

/// The gossipsub knobs this experiment varies.
///
/// Flood publishing is the trade-off under test: with it on, the victim sends
/// the message straight to every subscriber, so censoring relays cannot stop
/// it; with it off, sender anonymity improves but the attackers only have to
/// control the victim's mesh to eclipse it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GossipConfig {
    pub(crate) flood_publish: bool,
    pub(crate) mesh: MeshConfig,
}

/// The gossipsub mesh band, mirroring the one in the anonymity experiment so
/// the two are configured over the same D.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeshConfig {
    pub(crate) n: usize,
    pub(crate) low: usize,
    pub(crate) high: usize,
    pub(crate) outbound_min: usize,
}

impl MeshConfig {
    /// gossipsub rejects a config unless both
    ///
    ///   mesh_outbound_min <= mesh_n_low <= mesh_n <= mesh_n_high
    ///   mesh_outbound_min * 2 <= mesh_n
    ///
    /// hold, and mesh_outbound_min defaults to 2, so a swept D below 4 fails at
    /// swarm construction unless it is derived from the band as well.
    pub(crate) fn new(n: usize, low: Option<usize>, high: Option<usize>) -> Self {
        let n = n.max(1);
        let low = low
            .unwrap_or_else(|| n.saturating_sub(2).max(1))
            .clamp(1, n);
        let high = high.unwrap_or(n * 2).max(n);
        let outbound_min = (n / 2).min(low).min(2);
        MeshConfig {
            n,
            low,
            high,
            outbound_min,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PeerOut {
    pub(crate) _prop_id: PeerId, //PeerId of the propagation_source
}

#[derive(Clone, Debug)]
pub(crate) enum ControlSignal {
    Run,
    Stop,
    Attack,
    Pub(PeerId),
}

#[derive(Clone, Debug)]
pub(crate) enum PubInterval {
    Fix(u64), //in seconds
    Random((usize, usize)),
}

// Implement FromStr for PubInterval to handle argument parsing
impl FromStr for PubInterval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(fix_value) = s.strip_prefix("fix=") {
            fix_value
                .parse::<u64>()
                .map(PubInterval::Fix)
                .map_err(|_| format!("Invalid number for Fix: '{fix_value}'"))
        } else if let Some(random_values) = s.strip_prefix("random=") {
            let parts: Vec<&str> = random_values.split(',').collect();
            if parts.len() == 2 {
                let start = parts[0]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid number: '{}'", parts[0]))?;
                let end = parts[1]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid number: '{}'", parts[1]))?;
                Ok(PubInterval::Random((start, end)))
            } else {
                Err(format!("Invalid format for Random: '{random_values}'"))
            }
        } else {
            Err(format!(
                "Invalid interval type: '{s}'. Use 'fix=<number>' or 'random=<start,end>'."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariants gossipsub's ConfigBuilder checks. Asserted here so a bad
    /// band names the broken relation instead of panicking during swarm setup.
    #[test]
    fn band_satisfies_gossipsub_invariants() {
        for n in 1..=64usize {
            for low in [None, Some(1), Some(n), Some(n * 2)] {
                for high in [None, Some(n), Some(n + 1)] {
                    let m = MeshConfig::new(n, low, high);
                    assert!(m.outbound_min <= m.low, "{m:?}");
                    assert!(m.low <= m.n, "{m:?}");
                    assert!(m.n <= m.high, "{m:?}");
                    assert!(m.outbound_min * 2 <= m.n, "{m:?}");
                }
            }
        }
    }

    /// A pinned band, used when D must be exactly the configured degree.
    #[test]
    fn a_pinned_band_keeps_its_edges() {
        for n in 1..=64usize {
            let m = MeshConfig::new(n, Some(n), Some(n));
            assert_eq!((m.low, m.n, m.high), (n, n, n), "{m:?}");
        }
    }
}
