//! The slots a member has used per epoch, kept in slots.json next to its key material. Every
//! sharing peer of the member reads the file and writes it back at once, so that two peers do
//! not pick the same slot. The AA refuses a slot's tag a second time in any case.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
        std::fs::remove_dir_all(dir).ok();
    }
}
