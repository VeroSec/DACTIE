pub(crate) use openmls_memory_storage::{MemoryStorage, MemoryStorageError};
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

#[derive(Default, Debug)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct OpenMlsRustCrypto {
    crypto: RustCrypto,
    key_store: MemoryStorage,
}

impl OpenMlsProvider for OpenMlsRustCrypto {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.key_store
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

impl OpenMlsRustCrypto {
    /// The key store's entries, saved together with the groups.
    pub fn keystore_values(&self) -> HashMap<Vec<u8>, Vec<u8>> {
        self.key_store.values.read().unwrap().clone()
    }

    pub fn restore_keystore(&mut self, values: HashMap<Vec<u8>, Vec<u8>>) {
        *self.key_store.values.write().unwrap() = values;
    }

    /// Reads a key store that an earlier version saved on its own.
    pub fn load_keystore(&mut self, path: PathBuf) -> Result<(), String> {
        let file = match File::open(path) {
            Ok(val) => val,
            Err(e) => return Err(e.to_string()),
        };
        self.key_store.load_from_file(&file)
    }
}
