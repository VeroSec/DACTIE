use std::str::FromStr;
use libp2p::PeerId;

#[derive(Debug)]
pub(crate) struct PeerOut {
    pub(crate) prop_id: PeerId            //PeerId of the propagation_source
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
    Random((usize,usize))
}

// Implement FromStr for PubInterval to handle argument parsing
impl FromStr for PubInterval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(fix_value) = s.strip_prefix("fix=") {
            fix_value.parse::<u64>()
                .map(PubInterval::Fix)
                .map_err(|_| format!("Invalid number for Fix: '{}'", fix_value))
        } else if let Some(random_values) = s.strip_prefix("random=") {
            let parts: Vec<&str> = random_values.split(',').collect();
            if parts.len() == 2 {
                let start = parts[0].parse::<usize>().map_err(|_| format!("Invalid number: '{}'", parts[0]))?;
                let end = parts[1].parse::<usize>().map_err(|_| format!("Invalid number: '{}'", parts[1]))?;
                Ok(PubInterval::Random((start, end)))
            } else {
                Err(format!("Invalid format for Random: '{}'", random_values))
            }
        } else {
            Err(format!("Invalid interval type: '{}'. Use 'fix=<number>' or 'random=<start,end>'.", s))
        }
    }
}