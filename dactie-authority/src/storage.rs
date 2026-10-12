use crate::storage::MyStorageError::{
    CreateMemkeyError, FileAccessError, GetNError, IDNotinGMLError, KeyPackageSenderError,
    NotEnoughPartialOpeningsError, SerializationError, SignatureNotinIDPairMapError,
    ThresholdNotSetError,
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
use thressig::structs::{EncShare, EncSharePairing, Grpkey, Mgrkey, Signature, Token};
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
    // Archives needed to open a signature. Fixed when the archive init phase ends, because
    // every member's shares are created for this value.
    #[serde(default)]
    threshold: Mutex<Option<usize>>,
    #[serde_as(as = "Mutex<HashMap<DisplayFromStr, _>>")]
    signature_enc_id_pairing_map: Mutex<HashMap<Signature, Vec<(usize, EncSharePairing)>>>,
    #[serde_as(as = "Mutex<HashMap<DisplayFromStr, _>>")]
    signature_enc_peer_pairing_map: Mutex<HashMap<Signature, Vec<(PeerId, EncSharePairing)>>>,
    /// Secret from which the AA derives one token key per epoch.
    #[serde(default = "FieldElement::random")]
    token_master: FieldElement,
    /// The PeerID each member registered from: the only PeerID the AA links to a name, and
    /// the one the member requests tokens from.
    #[serde(default)]
    registration_peers: Mutex<HashMap<PeerId, usize>>,
    /// Tokens signed per member: MemberID -> (epoch, count).
    #[serde(default)]
    tokens_issued: Mutex<HashMap<usize, (u64, usize)>>,
    /// Serials of spent tokens, so that no token registers two PeerIDs.
    #[serde(default)]
    spent_tokens: Mutex<HashSet<Vec<u8>>>,
    /// (epoch length in seconds, tokens per member and epoch), set from the command line.
    #[serde(skip)]
    token_policy: Mutex<(u64, usize)>,
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
                token_master: FieldElement::random(),
                registration_peers: Mutex::new(HashMap::new()),
                tokens_issued: Mutex::new(HashMap::new()),
                spent_tokens: Mutex::new(HashSet::new()),
                token_policy: Mutex::new((30 * 24 * 3600, 5)),
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

    pub(crate) fn set_token_policy(&self, epoch_secs: u64, per_member: usize) {
        *self.token_policy.lock().unwrap() = (epoch_secs.max(1), per_member);
    }

    pub(crate) fn current_epoch(&self) -> u64 {
        let epoch_secs = self.token_policy.lock().unwrap().0;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now / epoch_secs
    }

    fn epoch_key(&self, epoch: u64) -> FieldElement {
        thressig::tokens::epoch_key(&self.token_master, epoch)
    }

    /// Signs blinded tokens for the member that registered from `peer`, at most k per member
    /// and epoch, and returns them with the epoch's public key. Refusals carry the current
    /// epoch, so a member that blinded for the wrong epoch can retry.
    pub(crate) fn issue_tokens(
        &self,
        peer: &PeerId,
        epoch: u64,
        blinded: &[G1],
    ) -> Result<(Vec<G1>, G2), (u64, String)> {
        let current = self.current_epoch();
        if epoch != current {
            return Err((current, format!("epoch {epoch} is not the current epoch")));
        }
        let member = *self
            .registration_peers
            .lock()
            .unwrap()
            .get(peer)
            .ok_or((current, "only a member's registration peer gets tokens".to_string()))?;
        let per_member = self.token_policy.lock().unwrap().1;
        let signed: Vec<G1> = {
            let mut issued = self.tokens_issued.lock().unwrap();
            let entry = issued.entry(member).or_insert((current, 0));
            if entry.0 != current {
                *entry = (current, 0);
            }
            let n = blinded.len().min(per_member - entry.1);
            entry.1 += n;
            let key = self.epoch_key(current);
            blinded[..n]
                .iter()
                .map(|b| thressig::tokens::sign_blinded(&key, b))
                .collect()
        };
        if signed.is_empty() {
            return Err((current, format!("all {per_member} tokens of this epoch were issued")));
        }
        self.to_file().map_err(|e| (current, e.to_string()))?;
        let pubkey = thressig::tokens::epoch_pubkey(&self.grpkey.g_tilde, &self.epoch_key(current));
        Ok((signed, pubkey))
    }

    /// Accepts each token once, and only in its own epoch.
    pub(crate) fn spend_token(&self, token: &Token) -> Result<(), String> {
        if token.epoch != self.current_epoch() {
            return Err(format!("token of epoch {} has expired", token.epoch));
        }
        if !thressig::tokens::verify_with_key(token, &self.epoch_key(token.epoch)) {
            return Err("token signature is invalid".to_string());
        }
        if !self.spent_tokens.lock().unwrap().insert(token.serial.clone()) {
            return Err("token was already spent".to_string());
        }
        self.to_file().map_err(|e| e.to_string())
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
        let registration_peer = *args.peer_id;
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
        self.registration_peers
            .lock()
            .unwrap()
            .insert(registration_peer, id);

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
    fn test_token_cap_and_spending() {
        let dir = std::env::temp_dir().join(format!("dactie-token-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (storage, _) = Storage::new(dir.join("storage.json"));
        storage.set_token_policy(3600, 3);
        let member = PeerId::random();
        storage.registration_peers.lock().unwrap().insert(member, 42);
        let epoch = storage.current_epoch();
        let blind = |n: usize| (0..n).map(|_| thressig::tokens::blind(epoch)).collect::<Vec<_>>();
        let points = |ts: &[thressig::structs::BlindedToken]| {
            ts.iter().map(|t| t.blinded.clone()).collect::<Vec<_>>()
        };

        // Only a member's registration peer, and only for the current epoch.
        assert!(storage.issue_tokens(&PeerId::random(), epoch, &points(&blind(1))).is_err());
        let refused = storage.issue_tokens(&member, epoch + 1, &points(&blind(1)));
        assert_eq!(refused.unwrap_err().0, epoch);

        // At most k = 3 per member and epoch, however they are requested.
        let first = blind(2);
        let (signed, pubkey) = storage.issue_tokens(&member, epoch, &points(&first)).unwrap();
        assert_eq!(signed.len(), 2);
        assert_eq!(storage.issue_tokens(&member, epoch, &points(&blind(5))).unwrap().0.len(), 1);
        assert!(storage.issue_tokens(&member, epoch, &points(&blind(1))).is_err());

        // Each token registers one PeerID, and only a token the AA signed counts.
        let token =
            thressig::tokens::unblind(&first[0], &signed[0], &storage.grpkey.g_tilde, &pubkey)
                .unwrap();
        assert!(storage.spend_token(&token).is_ok());
        assert!(storage.spend_token(&token).is_err());
        let forged = Token {
            serial: vec![7; 32],
            ..token
        };
        assert!(storage.spend_token(&forged).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
