//! The slots a member has used per epoch, kept in slots.json next to its key material. Every
//! sharing peer of the member reads the file and writes it back under a lock, so that two peers
//! do not pick the same slot. The AA refuses a slot's tag a second time in any case.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
pub struct SlotStore {
    used: BTreeMap<u64, Vec<u32>>,
}

impl SlotStore {
    fn path(dir: &Path) -> PathBuf {
        dir.join("slots.json")
    }

    /// No file means no slot used yet. A file that cannot be read is an error: starting over
    /// would hand out slots again, which the AA refuses.
    pub fn load(dir: &Path) -> io::Result<SlotStore> {
        match std::fs::read(Self::path(dir)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SlotStore::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        crate::persist::write_atomic(&Self::path(dir), &bytes)
    }

    /// Claims a slot of `epoch` in the slots.json in `dir` and records it, see claim. Sharing
    /// peers of one member may do this at the same moment, e.g. when they renew, so the file
    /// stays locked (slots.json.lock) from reading to writing; otherwise both could read the
    /// same slots and pick the same one.
    pub fn claim_in(dir: &Path, epoch: u64, k: usize) -> io::Result<Option<u32>> {
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("slots.json.lock"))?;
        lock.lock()?;
        let mut slots = Self::load(dir)?;
        let slot = slots.claim(epoch, k);
        if slot.is_some() {
            slots.save(dir)?;
        }
        Ok(slot)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_file_is_an_error_not_a_reset() {
        let dir = std::env::temp_dir().join(format!("dactie-slots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No file yet: no slot used.
        let mut slots = SlotStore::load(&dir).unwrap();
        assert_eq!(slots.claim(7, 2), Some(1));
        slots.save(&dir).unwrap();
        assert_eq!(SlotStore::load(&dir).unwrap().claim(7, 2), Some(2));

        std::fs::write(dir.join("slots.json"), b"{\"used\": {\"7\": [1").unwrap();
        assert!(SlotStore::load(&dir).is_err());
        assert!(SlotStore::claim_in(&dir, 7, 2).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Peers that claim at the same moment get different slots.
    #[test]
    fn concurrent_claims_get_different_slots() {
        let dir = std::env::temp_dir().join(format!("dactie-slots-race-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let claims: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || SlotStore::claim_in(&dir, 3, 8).unwrap().unwrap())
            })
            .collect();
        let mut slots: Vec<u32> = claims.into_iter().map(|c| c.join().unwrap()).collect();
        slots.sort();
        assert_eq!(slots, (1..=8).collect::<Vec<_>>());
        assert_eq!(SlotStore::claim_in(&dir, 3, 8).unwrap(), None);
        std::fs::remove_dir_all(dir).ok();
    }
}
