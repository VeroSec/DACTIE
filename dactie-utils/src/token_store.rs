//! A member's registration tokens, kept in tokens.json next to its key material. The
//! registration peer adds tokens and every new sharing peer (peer_from_sig) takes one, so both
//! read the file and write it back at once instead of keeping a copy in memory.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use thressig::structs::{BlindedToken, Token};

#[derive(Default, Serialize, Deserialize)]
pub struct TokenStore {
    /// The AA's current epoch as last learned from it.
    pub epoch: u64,
    /// Blinded tokens sent to the AA and not yet answered.
    pub pending: Vec<BlindedToken>,
    /// Unspent tokens.
    pub tokens: Vec<Token>,
}

impl TokenStore {
    fn path(dir: &Path) -> PathBuf {
        dir.join("tokens.json")
    }

    pub fn load(dir: &Path) -> TokenStore {
        File::open(Self::path(dir))
            .ok()
            .and_then(|file| serde_json::from_reader(file).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let tmp = dir.join("tokens.json.tmp");
        serde_json::to_writer_pretty(File::create(&tmp)?, self).map_err(io::Error::other)?;
        std::fs::rename(tmp, Self::path(dir))
    }

    /// Removes and returns a token of `epoch`.
    pub fn take(&mut self, epoch: u64) -> Option<Token> {
        let i = self.tokens.iter().position(|t| t.epoch == epoch)?;
        Some(self.tokens.remove(i))
    }
}
