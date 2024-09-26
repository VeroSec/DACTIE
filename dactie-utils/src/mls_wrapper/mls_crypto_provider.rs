use std::fs::File;
use std::path::PathBuf;
pub(crate) use openmls_memory_storage::{MemoryStorage, MemoryStorageError};
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;

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
    pub fn save_keystore(&self, path: PathBuf) -> Result<(), String> {
        let file = match  File::create(path){
            Ok(val) => val,
            Err(e) => return Err(e.to_string())
        };
        self.key_store.save_to_file(&file)
    }

    pub fn load_keystore(&mut self, path: PathBuf) -> Result<(), String> {
        let file = match  File::open(path){
            Ok(val) => val,
            Err(e) => return Err(e.to_string())
        };
        self.key_store.load_from_file(&file)
    }
}
