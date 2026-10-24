use crate::storage::MyStorageError::{
    CreateMemkeyError, FileAccessError, GetNError, IDNotinGMLError, KeyPackageSenderError,
    NotEnoughPartialOpeningsError, SerializationError, SignatureNotinIDPairMapError,
    ThresholdNotSetError,
};
use dactie_utils::key_store::StoredKeyMaterial;
use dactie_utils::mls_wrapper::{key_package_belongs_to, MyOpenMls};
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
use thressig::structs::{EncShare, EncSharePairing, Grpkey, Mgrkey, Signature, SlotTag};
use thressig::{FieldElement, GroupElement, G1, G2};

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
    // Archives needed to open a signature. Fixed when the archive init phase ends, because
    // every member's shares are created for this value.
    #[serde(default)]
    threshold: Mutex<Option<usize>>,
    #[serde_as(as = "Mutex<HashMap<DisplayFromStr, _>>")]
    signature_enc_id_pairing_map: Mutex<HashMap<Signature, Vec<(usize, EncSharePairing)>>>,
    #[serde_as(as = "Mutex<HashMap<DisplayFromStr, _>>")]
    signature_enc_peer_pairing_map: Mutex<HashMap<Signature, Vec<(PeerId, EncSharePairing)>>>,
    /// Each registered sharing peer and the last epoch its registration covers.
    #[serde(default)]
    peer_epochs: Mutex<HashMap<PeerId, u64>>,
    /// Slot tags already used, per epoch, so that no tag registers or renews a peer twice.
    #[serde(default)]
    used_tags: Mutex<HashMap<u64, HashSet<Vec<u8>>>>,
    /// (epoch length in seconds, slots per member and epoch), set from the command line.
    #[serde(skip)]
    epoch_policy: Mutex<(u64, usize)>,
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
    pub pi3: (FieldElement, FieldElement),
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
                threshold: Mutex::new(None),
                signature_enc_id_pairing_map: Mutex::new(HashMap::new()),
                signature_enc_peer_pairing_map: Mutex::new(HashMap::new()),
                peer_epochs: Mutex::new(HashMap::new()),
                used_tags: Mutex::new(HashMap::new()),
                epoch_policy: Mutex::new((30 * 24 * 3600, 5)),
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

    pub(crate) fn get_threshold(&self) -> Option<usize> {
        *self.threshold.lock().unwrap()
    }

    pub(crate) fn set_threshold(&self, threshold: usize) {
        *self.threshold.lock().unwrap() = Some(threshold);
    }

    /// Distinct archives that have contributed a partial opening for this signature.
    fn contributing_archives(&self, signature: &Signature) -> usize {
        self.signature_enc_id_pairing_map
            .lock()
            .unwrap()
            .get(signature)
            .map(|pairings| {
                pairings
                    .iter()
                    .map(|(_, p)| p.index)
                    .collect::<HashSet<_>>()
                    .len()
            })
            .unwrap_or(0)
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

    // Adds new Peer to network
    pub(crate) fn allow_peer(
        &self,
        key_package: &KeyPackage,
        sender_peer_id: PeerId,
    ) -> Result<(), MyStorageError> {
        if key_package_belongs_to(key_package, sender_peer_id) {
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

    //Retrieves all EncShares of a signature
    fn open_ids(
        &self,
        signature: &Signature,
        my_open_mls: &MyOpenMls,
    ) -> Result<Vec<(usize, String)>, MyStorageError> {
        let enc_share_pairings = self
            .signature_enc_id_pairing_map
            .lock()
            .unwrap()
            .get(signature)
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

    //Retrieves all EncShares of a signature
    fn open_peers(
        &self,
        signature: &Signature,
        my_open_mls: &MyOpenMls,
    ) -> Result<Vec<PeerId>, MyStorageError> {
        let enc_share_pairings = self
            .signature_enc_peer_pairing_map
            .lock()
            .unwrap()
            .get(signature)
            .cloned()
            .unwrap_or_default();
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
        // Fewer partial openings than the threshold cannot reconstruct anything, so keep
        // collecting until enough archives have approved.
        let need = self.get_threshold().ok_or(ThresholdNotSetError)?;
        let have = self.contributing_archives(&signature);
        if have < need {
            return Err(NotEnoughPartialOpeningsError { have, need });
        }
        let res = (
            self.open_ids(&signature, my_open_mls)?,
            self.open_peers(&signature, my_open_mls)?,
        );
        if !res.0.is_empty() {
            self.signature_enc_id_pairing_map.lock().unwrap().remove(&signature);
            self.signature_enc_peer_pairing_map.lock().unwrap().remove(&signature);
        }
        self.to_file()?;
        Ok(res)
    }

    pub(crate) fn set_epoch_policy(&self, epoch_secs: u64, per_member: usize) {
        *self.epoch_policy.lock().unwrap() = (epoch_secs.max(1), per_member);
    }

    /// (epoch length in seconds, slots per member and epoch).
    pub(crate) fn get_epoch_policy(&self) -> (u64, usize) {
        *self.epoch_policy.lock().unwrap()
    }

    pub(crate) fn current_epoch(&self) -> u64 {
        let epoch_secs = self.epoch_policy.lock().unwrap().0;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now / epoch_secs
    }

    /// Accepts a slot tag once, for a slot in 1..=k and one of `epochs`. The proof that the
    /// tag belongs to a member must have been checked before.
    fn accept_tag(&self, tag: &SlotTag, epochs: &[u64]) -> Result<(), String> {
        let current = self.current_epoch();
        let per_member = self.epoch_policy.lock().unwrap().1;
        if !epochs.contains(&tag.epoch) {
            return Err(format!("slot tag of epoch {} is not valid in epoch {current}", tag.epoch));
        }
        if tag.slot == 0 || tag.slot as usize > per_member {
            return Err(format!("slot {} is not one of the {per_member} slots", tag.slot));
        }
        {
            let mut used = self.used_tags.lock().unwrap();
            used.retain(|epoch, _| *epoch >= current);
            if !used
                .entry(tag.epoch)
                .or_default()
                .insert(tag.tag.to_bytes(true))
            {
                return Err(format!(
                    "slot {} of epoch {} was already used by this member",
                    tag.slot, tag.epoch
                ));
            }
        }
        self.to_file().map_err(|e| e.to_string())
    }

    /// A new sharing peer needs a tag of the current epoch.
    pub(crate) fn accept_registration_tag(&self, tag: &SlotTag) -> Result<(), String> {
        self.accept_tag(tag, &[self.current_epoch()])
    }

    /// Records that a sharing peer's registration covers `epoch`.
    pub(crate) fn register_peer_epoch(&self, peer: PeerId, epoch: u64) -> Result<(), MyStorageError> {
        self.peer_epochs.lock().unwrap().insert(peer, epoch);
        self.to_file()
    }

    /// Extends a registered sharing peer to the tag's epoch, the current or the next one.
    pub(crate) fn renew_peer(&self, peer: &PeerId, tag: &SlotTag) -> Result<u64, String> {
        let current = self.current_epoch();
        if !self.peer_epochs.lock().unwrap().contains_key(peer) {
            return Err("not a registered sharing peer".to_string());
        }
        self.accept_tag(tag, &[current, current + 1])?;
        let until = {
            let mut epochs = self.peer_epochs.lock().unwrap();
            let until = epochs.entry(*peer).or_insert(0);
            *until = (*until).max(tag.epoch);
            *until
        };
        self.to_file().map_err(|e| e.to_string())?;
        Ok(until)
    }

    /// Sharing peers whose registration ended with an earlier epoch.
    pub(crate) fn expired_peers(&self) -> Vec<PeerId> {
        let current = self.current_epoch();
        self.peer_epochs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, until)| **until < current)
            .map(|(peer, _)| *peer)
            .collect()
    }

    /// Forgets expired peers. They stay in allowed_peers so old signatures can still be
    /// opened to them.
    pub(crate) fn forget_peers(&self, peers: &[PeerId]) -> Result<(), MyStorageError> {
        {
            let mut epochs = self.peer_epochs.lock().unwrap();
            for peer in peers {
                epochs.remove(peer);
            }
        }
        self.to_file()
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
                pi3: args.pi3,
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

    #[error("Only {have} of the {need} archives needed have approved this opening")]
    NotEnoughPartialOpeningsError { have: usize, need: usize },

    #[error("The opening threshold is not set; end the archive init phase first")]
    ThresholdNotSetError,

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_tags_renewal_and_expiry() {
        let dir = std::env::temp_dir().join(format!("dactie-slot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (storage, _) = Storage::new(dir.join("storage.json"));
        storage.set_epoch_policy(3600, 2);
        let epoch = storage.current_epoch();
        let sk = FieldElement::random();
        let tag = |e, s| thressig::tags::tag(&sk, e, s);

        // A new peer needs one of the k = 2 slots of the current epoch, each once.
        assert!(storage.accept_registration_tag(&tag(epoch, 1)).is_ok());
        assert!(storage.accept_registration_tag(&tag(epoch, 1)).is_err());
        assert!(storage.accept_registration_tag(&tag(epoch, 3)).is_err());
        assert!(storage.accept_registration_tag(&tag(epoch + 1, 2)).is_err());
        assert!(storage.accept_registration_tag(&tag(epoch, 2)).is_ok());
        // Another member has its own tags for the same slots.
        let other = thressig::tags::tag(&FieldElement::random(), epoch, 1);
        assert!(storage.accept_registration_tag(&other).is_ok());

        // Renewal: registered peers only, with a tag of the current or the next epoch.
        let (stays, leaves, stranger) = (PeerId::random(), PeerId::random(), PeerId::random());
        storage.register_peer_epoch(stays, epoch).unwrap();
        storage.register_peer_epoch(leaves, epoch - 1).unwrap();
        assert!(storage.renew_peer(&stranger, &tag(epoch + 1, 1)).is_err());
        assert_eq!(storage.renew_peer(&stays, &tag(epoch + 1, 1)).unwrap(), epoch + 1);
        assert!(storage.renew_peer(&stays, &tag(epoch + 1, 1)).is_err());
        assert!(storage.renew_peer(&stays, &tag(epoch + 2, 1)).is_err());

        // Only the peer whose registration ended before the current epoch expires.
        assert_eq!(storage.expired_peers(), vec![leaves]);
        storage.forget_peers(&[leaves]).unwrap();
        assert!(storage.expired_peers().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
