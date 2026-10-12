//! The slots a member has used per epoch, kept in slots.json next to its key material. Every
//! sharing peer of the member reads the file and writes it back at once, so that two peers do
//! not pick the same slot. The AA refuses a slot's tag a second time in any case.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
pub struct SlotStore {
    used: BTreeMap<u64, Vec<u32>>,
}

impl SlotStore {
    fn path(dir: &Path) -> PathBuf {
        dir.join("slots.json")
    }

    pub fn load(dir: &Path) -> SlotStore {
        File::open(Self::path(dir))
            .ok()
            .and_then(|file| serde_json::from_reader(file).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let tmp = dir.join("slots.json.tmp");
        serde_json::to_writer_pretty(File::create(&tmp)?, self).map_err(io::Error::other)?;
        std::fs::rename(tmp, Self::path(dir))
    }

    /// Claims the lowest unused slot of `epoch` in 1..=k, and forgets epochs that ended
    /// before the previous one.
    pub fn claim(&mut self, epoch: u64, k: usize) -> Option<u32> {
        self.used.retain(|e, _| *e + 1 >= epoch);
        let used = self.used.entry(epoch).or_default();
        let slot = (1..=k as u32).find(|s| !used.contains(s))?;
        used.push(slot);
        Some(slot)
    }
}
