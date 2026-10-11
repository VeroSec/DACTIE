use crate::storage::MyStorageError::{
    CreateMemkeyError, FileAccessError, GetNError, IDNotinGMLError, KeyPackageSenderError,
    SerializationError, SignatureNotinIDPairMapError, SignatureNotinPeerPairMapError,
};
use dactie_utils::key_store::StoredKeyMaterial;
use dactie_utils::mls_wrapper::MyOpenMls;
use libp2p::identity::ed25519::PublicKey;
use libp2p::PeerId;
use openmls::prelude::KeyPackage;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Mutex;
use thiserror::Error;
use thressig::structs::{EncShare, EncSharePairing, Grpkey, Mgrkey, Signature};
use thressig::{FieldElement, G1, G2};

/**
   The storage is used to save information about the added group members and the key material that is required to add new members.
**/
#[serde_as]
#[derive(Serialize, Deserialize)]
pub(crate) struct Storage {
    n_store: Mutex<HashMap<PeerId, (G1, usize, String)>>,
    grpkey: Grpkey,
    mgrkey: Mgrkey,
    gml: Mutex<HashMap<usize, String>>, //ID to Register_name
    path_buf: PathBuf,
    archive_peers: Mutex<Vec<PeerId>>,
    allowed_peers: Mutex<Vec<PeerId>>,
    archive_init_mode: Mutex<bool>,
    opener_pubkeys: Mutex<Vec<G2>>,
    #[serde_as(as = "Mutex<HashMap<DisplayFromStr, _>>")]
    signature_enc_id_pairing_map: Mutex<HashMap<Signature, Vec<(usize, EncSharePairing)>>>,
    #[serde_as(as = "Mutex<HashMap<DisplayFromStr, _>>")]
    signature_enc_peer_pairing_map: Mutex<HashMap<Signature, Vec<(PeerId, EncSharePairing)>>>,
}

type OpenSignatureResult = (Vec<(usize, String)>, Vec<PeerId>);

pub(crate) struct ReturnMemkeyArgs<'a> {
    pub peer_id: &'a PeerId,
    pub g_sk: &'a G1,
    pub h_sk: &'a G1,
    pub commits: &'a [G1],
    pub enc_shares: &'a [EncShare],
    pub pi1: (FieldElement, FieldElement),
    pub pi2: (FieldElement, FieldElement),
}

impl Storage {
    pub(crate) fn new(path_buf: PathBuf) -> (Storage, StoredKeyMaterial) {
        let (grpkey, mgrkey) = thressig::setup();

        let gml = HashMap::new();

        let key_material = StoredKeyMaterial::new(Some(grpkey.clone()), None, None, None);

        (
            Storage {
                n_store: Mutex::new(HashMap::new()),
                grpkey,
                mgrkey,
                gml: Mutex::new(gml),
                path_buf,
                allowed_peers: Mutex::new(Vec::new()),
                archive_peers: Mutex::new(Vec::new()),
                archive_init_mode: Mutex::new(true),
                opener_pubkeys: Mutex::new(Vec::new()),
                signature_enc_id_pairing_map: Mutex::new(HashMap::new()),
                signature_enc_peer_pairing_map: Mutex::new(HashMap::new()),
            },
            key_material,
        )
    }

    pub(crate) fn id_exists(&self, id: &usize) -> bool {
        self.gml.lock().unwrap().contains_key(id)
    }

    pub(crate) fn get_archive_peer_ids(&self) -> Vec<PeerId> {
        self.archive_peers.lock().unwrap().clone()
    }

    pub(crate) fn end_init_archive(&self) {
        let mut mode = self.archive_init_mode.lock().unwrap();
        *mode = false;
    }

    pub(crate) fn get_opener_pubkeys(&self) -> Vec<G2> {
        self.opener_pubkeys.lock().unwrap().clone()
    }

    /// Archive keys paired with the archive's PeerId, in the order shares are sent out.
    pub(crate) fn get_archive_keys(&self) -> Vec<(PeerId, G2)> {
        let archives = self.archive_peers.lock().unwrap();
        let keys = self.opener_pubkeys.lock().unwrap();
        archives.iter().copied().zip(keys.iter().cloned()).collect()
    }

    /// Registers an archive and its key together, so the two lists cannot drift apart when
    /// an archive registers twice.
    pub(crate) fn add_archive_key(&self, peer_id: PeerId, opener_pubkey: G2) {
        let mut archives = self.archive_peers.lock().unwrap();
        let mut keys = self.opener_pubkeys.lock().unwrap();
        match archives.iter().position(|p| *p == peer_id) {
            Some(i) => {
                log::warn!("Archive {peer_id} registered again, replacing its opener key");
                keys[i] = opener_pubkey;
            }
            None => {
                archives.push(peer_id);
                keys.push(opener_pubkey);
            }
        }
    }

    pub(crate) fn verify_key_package(
        key_package: &KeyPackage,
        sender_peer_id: PeerId,
    ) -> Result<bool, MyStorageError> {
        let pub_mls_key = key_package.leaf_node().signature_key().as_slice();
        let peer_id = libp2p::identity::PublicKey::from(
            PublicKey::try_from_bytes(pub_mls_key).map_err(|_| SerializationError {
                reason: "PeerID".to_string(),
            })?,
        )
        .to_peer_id();

        Ok(peer_id == sender_peer_id)
    }

    // Adds new Peer to network
    pub(crate) fn allow_peer(
        &self,
        key_package: &KeyPackage,
        sender_peer_id: PeerId,
    ) -> Result<(), MyStorageError> {
        if Self::verify_key_package(key_package, sender_peer_id)? {
            {
                let mut vec = self.allowed_peers.lock().unwrap();
                if !vec.contains(&sender_peer_id) {
                    vec.push(sender_peer_id)
                }
            }
        } else {
            return Err(KeyPackageSenderError {
                sender_peer_id: Box::new(sender_peer_id),
                peer_id: Box::new(sender_peer_id),
            });
        }
        self.to_file()?;
        Ok(())
    }

    //Stores a Slice of EncSharePairings for further use
    pub fn insert_enc_id_share_pairing(
        &self,
        signature: Signature,
        enc_share_id_pairings: &[(usize, EncSharePairing)],
    ) {
        let mut map = self.signature_enc_id_pairing_map.lock().unwrap();
        map.entry(signature)
            .or_default()
            .extend_from_slice(enc_share_id_pairings);
    }

    //Stores a Slice of EncSharePairings for further use
    pub fn insert_enc_peer_share_pairing(
        &self,
        signature: Signature,
        enc_share_peer_pairings: &[(PeerId, EncSharePairing)],
    ) {
        let mut map = self.signature_enc_peer_pairing_map.lock().unwrap();
        map.entry(signature)
            .or_default()
            .extend_from_slice(enc_share_peer_pairings);
    }

    //Retrieves all EncShares of a signature, This function removes the signature from the pairing map
    fn open_ids(
        &self,
        signature: &Signature,
        my_open_mls: &MyOpenMls,
    ) -> Result<Vec<(usize, String)>, MyStorageError> {
        let enc_share_pairings = self
            .signature_enc_id_pairing_map
            .lock()
            .unwrap()
            .remove(signature)
            .ok_or(SignatureNotinIDPairMapError)?
            .clone();

        let all_sj: HashMap<usize, Vec<(usize, thressig::GT)>> = enc_share_pairings
            .into_iter()
            .fold(HashMap::new(), |mut map, (id, share)| {
                map.entry(share.index)
                    .or_default()
                    .push((id, share.pairing));
                map
            });
        let gml = self.gml.lock().unwrap();
        let keys = gml.keys().copied().collect();

        let mut result = Vec::new();

        for t in my_open_mls.open(signature, keys, all_sj).iter() {
            let id = *t;
            match gml.get(&id).cloned() {
                Some(value) => result.push((id, value)),
                None => return Err(IDNotinGMLError { id }),
            }
        }

        Ok(result)
    }

    //Retrieves all EncShares of a signature. This function removes the signature from the pairing map
    fn open_peers(
        &self,
        signature: &Signature,
        my_open_mls: &MyOpenMls,
    ) -> Result<Vec<PeerId>, MyStorageError> {
        let enc_share_pairings = self
            .signature_enc_peer_pairing_map
            .lock()
            .unwrap()
            .remove(signature)
            .ok_or(SignatureNotinPeerPairMapError)?
            .clone();
        let gml = HashSet::from_iter(self.allowed_peers.lock().unwrap().clone());

        let all_sj: HashMap<usize, Vec<(PeerId, thressig::GT)>> = enc_share_pairings
            .into_iter()
            .fold(HashMap::new(), |mut map, (id, share)| {
                map.entry(share.index)
                    .or_default()
                    .push((id, share.pairing));
                map
            });
        Ok(my_open_mls.open(signature, gml, all_sj))
    }

    pub fn open_signature(
        &self,
        signature: Signature,
        my_open_mls: &MyOpenMls,
    ) -> Result<OpenSignatureResult, MyStorageError> {
        let res = (
            self.open_ids(&signature, my_open_mls)?,
            self.open_peers(&signature, my_open_mls)?,
        );
        self.to_file()?;
        Ok(res)
    }

    pub fn remove_n(&self, peer_id: &PeerId) -> Option<(G1, usize, String)> {
        self.n_store.lock().unwrap().remove(peer_id)
    }

    //Generates n and stores it and returns it with a grpkey
    pub(crate) fn return_new_n(
        &self,
        peer_id: PeerId,
        id: usize,
        name: String,
    ) -> (Option<G1>, Vec<u8>) {
        let n = &thressig::join_mgr1();
        self.n_store
            .lock()
            .unwrap()
            .insert(peer_id, (n.clone(), id, name));
        (Some(n.clone()), self.grpkey.to_bytes())
    }

    pub(crate) fn get_archive_init_mode(&self) -> bool {
        let mode = self.archive_init_mode.lock().unwrap();
        *mode
    }

    //Creates a Memkey from the member material
    pub(crate) fn return_memkey(
        &self,
        args: ReturnMemkeyArgs,
    ) -> Result<(usize, Vec<u8>), MyStorageError> {
        let (n, id, name) = self.remove_n(args.peer_id).ok_or(GetNError {
            peer_id: *args.peer_id,
        })?;
        let opener_pubkeys = self.opener_pubkeys.lock().unwrap().clone();
        let mut gml_guard = self.gml.lock().unwrap();
        let part_memkey = thressig::join_mgr2(
            id,
            thressig::JoinMgr2Args {
                gml: &mut gml_guard,
                name,
                mgrkey: &self.mgrkey,
                grpkey: &self.grpkey,
                n: &n,
                g_sk: args.g_sk,
                h_sk: args.h_sk,
                opener_pubkeys: &opener_pubkeys,
                commits: args.commits,
                pi1: args.pi1,
                pi2: args.pi2,
                enc_shares: args.enc_shares,
            },
        )
        .map_err(|reason| CreateMemkeyError { reason })?;

        // to_file() serializes every field of Storage, including gml. std::sync::Mutex
        // is not reentrant, so the guard must be released first or serialization
        // deadlocks against it, leaving a half-written file on disk.
        drop(gml_guard);

        self.to_file()?;
        Ok((id, part_memkey.to_bytes()))
    }

    /**
    Functions to save storage to a file.
    **/
    pub(crate) fn to_file(&self) -> Result<(), MyStorageError> {
        // Serialize to a sibling temp file and rename over the target. File::create
        // truncates immediately, so writing in place turns any mid-write failure into
        // an unparseable storage.json that the authority cannot start from.
        let mut tmp_path = self.path_buf.clone();
        let mut name = tmp_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "storage.json".to_string());
        name.push_str(".tmp");
        tmp_path.set_file_name(name);

        let file = File::create(&tmp_path).map_err(|e| FileAccessError {
            reason: e.to_string(),
        })?;
        serde_json::to_writer_pretty(&file, self).map_err(|e| SerializationError {
            reason: e.to_string(),
        })?;
        file.sync_all().map_err(|e| FileAccessError {
            reason: e.to_string(),
        })?;
        std::fs::rename(&tmp_path, &self.path_buf).map_err(|e| FileAccessError {
            reason: e.to_string(),
        })?;
        Ok(())
    }

    pub(crate) fn from_file(path: PathBuf) -> Result<Storage, MyStorageError> {
        let file = File::open(path).map_err(|e| FileAccessError {
            reason: e.to_string(),
        })?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(|e| SerializationError {
            reason: e.to_string(),
        })
    }
}
#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
pub enum MyStorageError {
    #[error("Failed create Memkey: `{reason:?}`")]
    CreateMemkeyError { reason: String },

    #[error("N for peer_id ´{peer_id:?}´ is not stored")]
    GetNError { peer_id: PeerId },

    #[error("ID {id} could not be found in GML")]
    IDNotinGMLError { id: usize },

    #[error("Signature could not be found in SignatureEncIDMap")]
    SignatureNotinIDPairMapError,

    #[error("Signature could not be found in SignatureEncPeerMap")]
    SignatureNotinPeerPairMapError,

    #[error("Got a keypackage with a different identity from a peer. Sender PeerID:{sender_peer_id}, KeyPackage PeerID: {peer_id}")]
    KeyPackageSenderError {
        sender_peer_id: Box<PeerId>,
        peer_id: Box<PeerId>,
    },

    #[error("Could not serialize/deserialize {reason:?}")]
    SerializationError { reason: String },

    #[error("Could not access file: {reason:?}")]
    FileAccessError { reason: String },
}
