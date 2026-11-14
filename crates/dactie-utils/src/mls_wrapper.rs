mod mls_crypto_provider;

use crate::key_store::{JoinKeyMaterial, StoredKeyMaterial};
use crate::persist;
use crate::shared_structs;
use crate::shared_structs::Instruction;
use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key,
};
use libp2p::PeerId;
use mls_crypto_provider::{MemoryStorageError, OpenMlsRustCrypto};
use openmls::framing::Sender::Member;
use openmls::prelude::{tls_codec::*, *};
use openmls::treesync::RatchetTree;
use openmls_basic_credential::SignatureKeyPair;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::create_dir_all;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;
use thiserror::Error;
use thressig::structs::{EncShare, EncSharePairing, MemberProof, Signature, SlotTag};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use MyOpenMLSError::*;

const NUM_CHANNELS: u64 = 10;
//NETWORK_DELAY in seconds, makes sure that all nodes have received the necessary proposals
const NETWORK_DELAY: u64 = 2;
// Files in group_saves/: the groups and the OpenMLS key store, saved together.
const GROUP_STATE: &str = "state";
// The two files that held them separately before.
const LEGACY_GROUPS: &str = "mstore";
const LEGACY_PROVIDER: &str = "provider";

type LoadState = (
    Mutex<Option<GroupId>>,
    Mutex<Vec<GroupId>>,
    Mutex<HashMap<GroupId, GroupStorage>>,
    OpenMlsRustCrypto,
);
type StoredGroupData = (Option<GroupId>, Vec<(MlsGroup, bool)>, Vec<Vec<u8>>);
type AddGroupMembersResult = (Vec<PeerId>, Vec<u8>, Option<Vec<u8>>, u64);
type EncryptedGroupKey = (Vec<u8>, Vec<u8>, Vec<u8>);

pub struct MyOpenMls {
    ciphersuite: Ciphersuite,
    signer: SignatureKeyPair,
    credential_with_key: CredentialWithKey,
    provider: OpenMlsRustCrypto,
    //All Stores should be stored together with the MLS Provider in one SQLDB for Produciton
    key_package_store: Mutex<HashMap<PeerId, KeyPackage>>,
    group_store: Mutex<HashMap<GroupId, GroupStorage>>,
    group_ids: Mutex<Vec<GroupId>>,
    key_store: Mutex<StoredKeyMaterial>,
    subscribed_topics: Mutex<HashMap<String, Vec<Vec<u8>>>>,
    group_commit_tx: mpsc::UnboundedSender<GroupId>,
    instruction_tx: mpsc::UnboundedSender<Instruction>,
    load_files_path: PathBuf,
    common_group: Mutex<Option<GroupId>>,
    authority_id: PeerId,
    opener_pubkeys: Mutex<Vec<thressig::G2>>,
    // Archives needed to open a signature, set by the authority for the whole network.
    opener_threshold: Mutex<Option<usize>>,
    // (epoch length in seconds, slots per member and epoch), announced by the authority.
    epoch_policy: Mutex<Option<(u64, usize)>>,
}

/// What `open_group` returns: the group number, the data and, for an attributed message, the
/// verified MemberID.
pub type OpenedGroupMessage = (Option<usize>, Vec<u8>, Option<usize>);

impl MyOpenMls {
    pub fn new(
        key_store: StoredKeyMaterial,
        group_commit_tx: mpsc::UnboundedSender<GroupId>,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
        load_files_path: PathBuf,
        authority_id: PeerId,
    ) -> Result<MyOpenMls, MyOpenMLSError> {
        let ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
        let mut group_path = load_files_path.clone();
        group_path.push("group_saves");

        let (common_group, group_ids, group_store, provider) =
            match Self::load_from_file(group_path.clone(), instruction_tx.clone())? {
                Some(state) => state,
                None => (
                    Mutex::new(None),
                    Mutex::new(Vec::new()),
                    Mutex::new(HashMap::new()),
                    OpenMlsRustCrypto::default(),
                ),
            };

        let (credential_with_key, signer) =
            generate_openmls_credentials(&provider, key_store.get_skp(), key_store.get_peer_id())?;

        Ok(MyOpenMls {
            ciphersuite,
            signer,
            credential_with_key,
            provider,
            key_package_store: Mutex::new(HashMap::new()),
            group_store,
            group_ids,
            key_store: Mutex::new(key_store),
            subscribed_topics: Mutex::new(HashMap::new()),
            group_commit_tx,
            instruction_tx,
            load_files_path,
            common_group,
            authority_id,
            opener_pubkeys: Mutex::new(Vec::new()),
            opener_threshold: Mutex::new(None),
            epoch_policy: Mutex::new(None),
        })
    }

    /// Writes the key store to key_material.json in this node's folder.
    fn save_key_store(&self) -> Result<(), MyOpenMLSError> {
        let path = self.load_files_path.join("key_material.json");
        self.key_store
            .lock()
            .unwrap()
            .to_file(path)
            .map_err(KeyMaterialWriteError)
    }

    pub fn generate_join_key_material(
        &self,
        id: usize,
        n: thressig::G1,
        grpkey: Vec<u8>,
    ) -> Result<JoinKeyMaterial, MyOpenMLSError> {
        let opener_pubkeys = self.opener_pubkeys.lock().unwrap().clone();
        let threshold = self.get_opener_threshold()?;
        let res = self
            .key_store
            .lock()
            .unwrap()
            .generate_join_key_material(id, n, grpkey, &opener_pubkeys, threshold)
            .map_err(InvalidKeyMaterial)?;
        self.save_key_store()?;
        Ok(res)
    }

    /// Shares the member key again for an additional sharing peer, takes the member's tag for
    /// `slot` of `epoch`, and proves that both hide the key of this member's credential.
    /// `context` binds the proof to the new peer.
    pub fn prepare_peer_registration(
        &self,
        context: &[u8],
        epoch: u64,
        slot: u32,
    ) -> Result<(Vec<EncShare>, Vec<thressig::G1>, SlotTag, MemberProof), MyOpenMLSError> {
        let opener_pubkeys = self.opener_pubkeys.lock().unwrap().clone();
        let mut key_store = self.key_store.lock().unwrap();
        let grpkey = key_store.get_grpkey().ok_or(GPKUndefinedError)?;
        let memkey = key_store.get_memkey().ok_or(GSKUndefinedError)?;
        let id = key_store.get_id().ok_or(IDUndefinedError)?;
        let threshold = self.get_opener_threshold()?;
        let (enc_shares, commits) = key_store.generate_enc_shares(
            grpkey.clone(),
            memkey.sk.clone(),
            id,
            &opener_pubkeys,
            threshold,
        );
        let h_blind = enc_shares
            .first()
            .ok_or(ElgamalUndefinedError)?
            .h_blind
            .clone();
        let tag = thressig::tags::tag(&memkey.sk, epoch, slot);
        let pairs = [
            (h_blind, commits[0].clone()),
            thressig::tags::statement(&tag),
        ];
        let proof = thressig::prove_member(&memkey, &grpkey, &pairs, context)?;
        Ok((enc_shares, commits, tag, proof))
    }

    /// Checks that a new peer's shares form one polynomial over the archives, and that they
    /// and the slot tag hide the key of some member's credential, without learning which member.
    pub fn verify_peer_registration(
        &self,
        proof: &MemberProof,
        tag: &SlotTag,
        enc_shares: &[EncShare],
        commits: &[thressig::G1],
        n_openers: usize,
        context: &[u8],
    ) -> bool {
        let grpkey = match self.key_store.lock().unwrap().get_grpkey() {
            Some(v) => v,
            None => return false,
        };
        match thressig::check_share_layout(enc_shares, commits, n_openers) {
            Ok(h_blind) => {
                let pairs = [
                    (h_blind, commits[0].clone()),
                    thressig::tags::statement(tag),
                ];
                thressig::verify_member(proof, &grpkey, &pairs, context)
            }
            Err(_) => false,
        }
    }

    /// The member's tag for `slot` of `epoch` and its proof, for renewing a sharing peer.
    pub fn prove_renewal(
        &self,
        context: &[u8],
        epoch: u64,
        slot: u32,
    ) -> Result<(SlotTag, MemberProof), MyOpenMLSError> {
        let key_store = self.key_store.lock().unwrap();
        let grpkey = key_store.get_grpkey().ok_or(GPKUndefinedError)?;
        let memkey = key_store.get_memkey().ok_or(GSKUndefinedError)?;
        let tag = thressig::tags::tag(&memkey.sk, epoch, slot);
        let proof = thressig::prove_member(
            &memkey,
            &grpkey,
            &[thressig::tags::statement(&tag)],
            context,
        )?;
        Ok((tag, proof))
    }

    pub fn verify_renewal(&self, proof: &MemberProof, tag: &SlotTag, context: &[u8]) -> bool {
        match self.key_store.lock().unwrap().get_grpkey() {
            Some(grpkey) => {
                thressig::verify_member(proof, &grpkey, &[thressig::tags::statement(tag)], context)
            }
            None => false,
        }
    }

    /// Epoch length and slots per member and epoch, as the AA announced them.
    pub fn set_epoch_policy(&self, epoch_secs: u64, peers_per_member: usize) {
        *self.epoch_policy.lock().unwrap() = Some((epoch_secs.max(1), peers_per_member));
    }

    /// The AA's current epoch and k, if this node has fetched them with get_opener_keys.
    pub fn epoch_policy(&self) -> Option<(u64, usize)> {
        let (epoch_secs, k) = (*self.epoch_policy.lock().unwrap())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Some((now / epoch_secs, k))
    }

    //Generate Keypair used for encryption of the shares
    pub fn generate_elgamal_key_pair(&self) -> Result<(), MyOpenMLSError> {
        self.key_store
            .lock()
            .unwrap()
            .generate_elgamal_key_pair()
            .map_err(|_| GPKUndefinedError)?;
        self.save_key_store()
    }

    pub fn get_elgamal_pubkey(&self) -> Result<thressig::G2, MyOpenMLSError> {
        Ok(self
            .key_store
            .lock()
            .unwrap()
            .get_elgamal_pubkey()
            .ok_or(ElgamalUndefinedError)?
            .clone())
    }

    pub fn get_peerid(&self) -> PeerId {
        self.key_store.lock().unwrap().get_peer_id()
    }

    pub fn get_authorityid(&self) -> PeerId {
        self.authority_id
    }

    pub fn set_grpkey(&self, grpkey: Vec<u8>) -> Result<(), MyOpenMLSError> {
        self.key_store
            .lock()
            .unwrap()
            .set_grpkey(grpkey)
            .map_err(InvalidKeyMaterial)?;
        self.save_key_store()
    }

    pub fn set_memkey(&self, memkey: Vec<u8>) -> Result<(), MyOpenMLSError> {
        self.key_store
            .lock()
            .unwrap()
            .set_memkey(memkey)
            .map_err(InvalidKeyMaterial)?;
        self.save_key_store()
    }

    pub fn set_id(&self, id: usize) -> Result<(), MyOpenMLSError> {
        self.key_store.lock().unwrap().set_id(id);
        self.save_key_store()
    }

    pub fn add_partial_memkey(&self, partial_memkey: Vec<u8>) -> Result<(), MyOpenMLSError> {
        self.key_store
            .lock()
            .unwrap()
            .add_partial_memkey(partial_memkey)
            .map_err(InvalidKeyMaterial)?;
        self.save_key_store()
    }

    pub fn set_opener_threshold(&self, threshold: usize) {
        *self.opener_threshold.lock().unwrap() = Some(threshold);
    }

    pub fn get_opener_threshold(&self) -> Result<usize, MyOpenMLSError> {
        self.opener_threshold
            .lock()
            .unwrap()
            .ok_or(ThresholdUndefinedError)
    }

    pub fn add_opener_pubkeys(
        &self,
        opener_pubkeys: &[thressig::G2],
    ) -> Result<(), MyOpenMLSError> {
        self.opener_pubkeys
            .lock()
            .unwrap()
            .extend_from_slice(opener_pubkeys);
        self.save_key_store()
    }

    pub fn store_to_file(&self) -> Result<(), MyOpenMLSError> {
        let map = self.group_store.lock().unwrap();
        let vec = self.subscribed_topics.lock().unwrap();
        let groups: Vec<_> = map.values().map(|x| (&x.group, &x.owned)).collect();
        let mut topics = Vec::new();
        for topic in vec.values() {
            topics.extend(topic);
        }
        let x = serde_json::to_vec(&(self.common_group.lock().unwrap().clone(), groups, topics))
            .map_err(|e| SerializationError(e.to_string()))?;
        let mut path = self.load_files_path.clone();
        path.push("group_saves");
        if !path.exists() {
            create_dir_all(path.clone())?;
        }
        // The groups and the OpenMLS key store describe the same groups and must match, so
        // they are written together in one file, which replaces the former mstore and provider.
        let state = bincode::serialize(&(x, self.provider.keystore_values()))
            .map_err(|e| SerializationError(e.to_string()))?;
        persist::write_atomic(&path.join(GROUP_STATE), &state)?;
        for legacy in [LEGACY_GROUPS, LEGACY_PROVIDER] {
            let _ = fs::remove_file(path.join(legacy));
        }

        Ok(())
    }

    /// Restores the saved groups, or returns None if this node never saved any. Saved state
    /// that cannot be read is an error: starting without it would silently drop the node's
    /// groups.
    fn load_from_file(
        dir: PathBuf,
        instruction_tx: mpsc::UnboundedSender<Instruction>,
    ) -> Result<Option<LoadState>, MyOpenMLSError> {
        let load_error = |reason: String| LoadError { reason };
        let mut provider = OpenMlsRustCrypto::default();
        let x = match fs::read(dir.join(GROUP_STATE)) {
            Ok(state) => {
                let (x, values): (Vec<u8>, HashMap<Vec<u8>, Vec<u8>>) =
                    bincode::deserialize(&state)
                        .map_err(|e| load_error(format!("{GROUP_STATE}: {e}")))?;
                provider.restore_keystore(values);
                x
            }
            // Saved by an earlier version, as two files.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs::read(dir.join(LEGACY_GROUPS)) {
                    Ok(x) => {
                        provider
                            .load_keystore(dir.join(LEGACY_PROVIDER))
                            .map_err(|e| load_error(format!("{LEGACY_PROVIDER}: {e}")))?;
                        x
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        };
        let (common_group, groups, topics): StoredGroupData =
            serde_json::from_slice(&x).map_err(|e| SerializationError(e.to_string()))?;
        let mut group_ids = Vec::new();
        let mut group_store = HashMap::new();

        for (group, owned) in groups {
            let group_id = group.group_id().clone();
            if let Some(comm) = common_group.clone() {
                if comm != group_id {
                    group_ids.push(group_id.clone());
                }
            }

            let secret = group.export_secret(&provider, "Test", "Test".as_bytes(), 32)?;
            let key = Key::<Aes256Gcm>::from_slice(&secret);
            let aes_cipher = Aes256Gcm::new(key);

            let gs = GroupStorage {
                group,
                aes_cipher,
                owned,
                added_pks: HashMap::new(),
            };
            group_store.insert(group_id.clone(), gs);
        }

        for topic in topics {
            instruction_tx
                .send(Instruction::Subscribe(topic))
                .map_err(|e| LoadError {
                    reason: e.to_string(),
                })?;
        }
        Ok(Some((
            Mutex::new(common_group),
            Mutex::new(group_ids),
            Mutex::new(group_store),
            provider,
        )))
    }

    /*
    Moves a Group to the common_group location
     */
    pub fn move_to_common(&self, group_number: usize) {
        let mut vec = self.group_ids.lock().unwrap();
        let mut common = self.common_group.lock().unwrap();
        *common = Some(vec.remove(group_number));
    }

    pub fn move_to_common_id(&self, group_id: Vec<u8>) -> Result<(), MyOpenMLSError> {
        let group_id = GroupId::from_slice(&group_id);
        let position = self
            .group_ids
            .lock()
            .unwrap()
            .iter()
            .position(|x| *x == group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;
        self.move_to_common(position);
        Ok(())
    }

    pub fn create_empty_mls_group(&self) -> Result<usize, MyOpenMLSError> {
        let group = MlsGroup::new(
            &self.provider,
            &self.signer,
            &MlsGroupCreateConfig::default(),
            self.credential_with_key.clone(),
        )
        .map_err(|e| CreateGroupError {
            reason: e.to_string(),
        })?;

        let secret = group
            .export_secret(&self.provider, "Test", "Test".as_bytes(), 32)
            .map_err(|e| CreateGroupError {
                reason: e.to_string(),
            })?;
        let key = Key::<Aes256Gcm>::from_slice(&secret);
        let aes_cipher = Aes256Gcm::new(key);

        let group_id = group.group_id().clone();

        {
            let mut map = self.group_store.lock().unwrap();
            map.insert(
                group_id.clone(),
                GroupStorage {
                    group,
                    aes_cipher,
                    owned: true,
                    added_pks: HashMap::new(),
                },
            );
        }

        let mut vec = self.group_ids.lock().unwrap();
        vec.push(group_id.clone());
        Ok(vec.len() - 1)
    }

    pub fn create_mls_group(&self, peer_ids: &Vec<PeerId>) -> Result<usize, MyOpenMLSError> {
        // Before the key packages are used up and an empty group is left behind.
        self.check_admitted(None, peer_ids.iter().copied())?;
        //Get Keypackages
        let pairs = self.get_keypackages_for_peer_ids(peer_ids)?;

        let group_numb = self.create_empty_mls_group()?;

        let (peer_ids, commit, welcome, epoch) =
            self.add_group_members(self.get_group_id(group_numb)?, &pairs)?;

        let group_id = self.get_group_id(group_numb)?;

        self.instruction_tx
            .send(Instruction::Commit {
                group_id,
                peer_ids,
                commit,
                welcome,
                epoch,
            })
            .map_err(|e| CreateGroupError {
                reason: e.to_string(),
            })?;

        Ok(group_numb)
    }

    pub fn remove_mls_group(&self, group_id_vec: Vec<u8>) {
        let group_id = GroupId::from_slice(&group_id_vec);
        let mut map = self.group_store.lock().unwrap();
        map.remove(&group_id);
        let mut vec = self.group_ids.lock().unwrap();
        vec.retain(|x| x.to_vec() != group_id_vec);
    }

    pub fn add_group_from_welcome(
        &self,
        serialized_welcome: Vec<u8>,
        serialized_ratchet_tree: Vec<u8>,
    ) -> Result<Vec<u8>, MyOpenMLSError> {
        // de-serialize the message as an [`MlsMessageIn`] ...
        let mls_message_in = MlsMessageIn::tls_deserialize(&mut serialized_welcome.as_slice())
            .map_err(|x| SerializationError(x.to_string()))?;
        let welcome = match mls_message_in.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            // The message comes from the network and can be anything.
            _ => {
                return Err(JoinGroupError {
                    reason: "not a Welcome message".to_string(),
                })
            }
        };

        // ... and inspect the message.
        let ratchet_tree: RatchetTree = bincode::deserialize(&serialized_ratchet_tree)
            .map_err(|x| SerializationError(x.to_string()))?;

        let staged_join = StagedWelcome::new_from_welcome(
            &self.provider,
            &MlsGroupJoinConfig::default(),
            welcome.clone(),
            // The public tree is need and transferred out of band.
            // It is also possible to use the [`RatchetTreeExtension`]
            Some(ratchet_tree.into()),
        )
        .map_err(|e| JoinGroupError {
            reason: e.to_string(),
        })?;

        let group = staged_join
            .into_group(&self.provider)
            .map_err(|e| JoinGroupError {
                reason: e.to_string(),
            })?;

        let secret = group.export_secret(&self.provider, "Test", "Test".as_bytes(), 32)?;
        let key = Key::<Aes256Gcm>::from_slice(&secret);
        let aes_cipher = Aes256Gcm::new(key);

        let group_id = group.group_id().clone();

        {
            let mut map = self.group_store.lock().unwrap();
            map.insert(
                group_id.clone(),
                GroupStorage {
                    group,
                    aes_cipher,
                    owned: false,
                    added_pks: HashMap::new(),
                },
            );
        }

        {
            let mut vec = self.group_ids.lock().unwrap();
            vec.push(group_id.clone());
        }

        Ok(group_id.to_vec())
    }

    pub fn get_group_id(&self, group_number: usize) -> Result<GroupId, MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        Ok(vec
            .get(group_number)
            .ok_or(GroupNotAvailable("".to_string()))?
            .clone())
    }

    /// Folder of this node's key material, which also holds its tokens.json.
    pub fn key_dir(&self) -> PathBuf {
        self.load_files_path.clone()
    }

    /// PeerIDs of all members of the common group, i.e. every admitted node.
    pub fn common_group_peers(&self) -> HashSet<PeerId> {
        let Ok(group_id) = self.get_common_group_id() else {
            return HashSet::new();
        };
        let map = self.group_store.lock().unwrap();
        map.get(&group_id)
            .map(|store| {
                store
                    .group
                    .members()
                    .filter_map(|m| PeerId::from_bytes(m.credential.serialized_content()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get_common_group_id(&self) -> Result<GroupId, MyOpenMLSError> {
        self.common_group
            .lock()
            .unwrap()
            .clone()
            .ok_or(CommonGroupUndefinedError)
    }

    /// Private groups are nested inside the common group: only its members can be added to
    /// them. `group_id` is the group they join, None for a group about to be created. The
    /// check needs this node's view of the common group, so it applies once the node has one;
    /// the AA creates the common group itself with create_mls_group.
    fn check_admitted(
        &self,
        group_id: Option<&GroupId>,
        peers: impl IntoIterator<Item = PeerId>,
    ) -> Result<(), MyOpenMLSError> {
        let Ok(common) = self.get_common_group_id() else {
            return Ok(());
        };
        if group_id == Some(&common) {
            return Ok(());
        }
        let admitted = self.common_group_peers();
        match peers.into_iter().find(|peer| !admitted.contains(peer)) {
            Some(peer_id) => Err(NotAdmittedError { peer_id }),
            None => Ok(()),
        }
    }

    /// Removes the given peers from every private group this node owns, with one commit per
    /// removed member, and returns (group number, peer) for each removal. Peers call it with
    /// the peers the AA just removed from the common group, so that a private group never
    /// keeps a member that the common group no longer has.
    pub fn remove_from_owned_groups(
        &self,
        removed: &HashSet<PeerId>,
    ) -> Result<Vec<(usize, PeerId)>, MyOpenMLSError> {
        let targets: Vec<(usize, PeerId)> = {
            let group_ids = self.group_ids.lock().unwrap();
            let map = self.group_store.lock().unwrap();
            group_ids
                .iter()
                .enumerate()
                .filter_map(|(number, id)| map.get(id).map(|store| (number, store)))
                .filter(|(_, store)| store.owned)
                .flat_map(|(number, store)| {
                    store
                        .group
                        .members()
                        .filter_map(|m| PeerId::from_bytes(m.credential.serialized_content()).ok())
                        .filter(|peer| removed.contains(peer))
                        .map(move |peer| (number, peer))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        for (number, peer) in &targets {
            self.remove_group_member(*number, *peer)?;
        }
        Ok(targets)
    }

    pub fn add_group_members(
        &self,
        group_id: GroupId,
        pairs: &[(PeerId, KeyPackage)],
    ) -> Result<AddGroupMembersResult, MyOpenMLSError> {
        self.check_admitted(Some(&group_id), pairs.iter().map(|(p, _)| *p))?;
        let mut map = self.group_store.lock().unwrap();
        let group_store = map
            .get_mut(&group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        let epoch = group_store.group.epoch().as_u64();

        let key_packages: Vec<KeyPackage> = pairs.iter().map(|(_, k)| k.clone()).collect();
        let peer_ids = pairs.iter().map(|(p, _)| *p).collect();

        let (mls_message_out, welcome_out, _) = group_store
            .group
            .add_members(&self.provider, &self.signer, &key_packages)
            .map_err(|e| CreateGroupError {
                reason: e.to_string(),
            })?;
        group_store
            .group
            .merge_pending_commit(&self.provider)
            .map_err(|e| CreateGroupError {
                reason: e.to_string(),
            })?;
        let commit = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;

        for (peer_id, key_package) in pairs {
            group_store
                .added_pks
                .insert(key_package.leaf_node().signature_key().clone(), *peer_id);
        }

        group_store.update_cipher(&self.provider)?;

        let welcome = Some(
            welcome_out
                .tls_serialize_detached()
                .map_err(|e| SerializationError(e.to_string()))?,
        );

        Ok((peer_ids, commit, welcome, epoch))
    }

    /// Removes members from a group this node owns and returns the commit to publish, like
    /// add_group_members. The AA uses it to drop expired sharing peers from the common group.
    pub fn remove_group_members(
        &self,
        group_id: GroupId,
        peer_ids: &[PeerId],
    ) -> Result<(Vec<u8>, u64), MyOpenMLSError> {
        let mut map = self.group_store.lock().unwrap();
        let group_store = map
            .get_mut(&group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let epoch = group_store.group.epoch().as_u64();
        let leaves: Vec<LeafNodeIndex> = group_store
            .group
            .members()
            .filter(|m| {
                peer_ids
                    .iter()
                    .any(|p| m.credential.serialized_content() == p.to_bytes())
            })
            .map(|m| m.index)
            .collect();
        if leaves.is_empty() {
            return Err(RemoveGroupMemberError);
        }
        let (mls_message_out, _, _) = group_store
            .group
            .remove_members(&self.provider, &self.signer, &leaves)
            .map_err(|_| RemoveGroupMemberError)?;
        let commit = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;
        group_store
            .group
            .merge_pending_commit(&self.provider)
            .map_err(|e| CreateGroupError {
                reason: e.to_string(),
            })?;
        group_store.update_cipher(&self.provider)?;
        Ok((commit, epoch))
    }

    pub fn propose_add_group_member(
        &self,
        group_number: usize,
        peer_id: PeerId,
    ) -> Result<(Vec<u8>, Vec<u8>, u64), MyOpenMLSError> {
        self.check_admitted(Some(&self.get_group_id(group_number)?), [peer_id])?;
        let key_package = self
            .get_keypackage(peer_id)
            .ok_or(MissingKeyPackagesError {
                peer_ids: vec![peer_id],
            })?;

        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec
            .get(group_number)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage {
            group,
            owned,
            added_pks,
            ..
        } = map
            .get_mut(group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        let epoch = group.epoch().as_u64();

        let (mls_message_out, _) =
            group.propose_add_member(&self.provider, &self.signer, &key_package)?;
        let serialized_message = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;

        added_pks.insert(key_package.leaf_node().signature_key().clone(), peer_id);
        self.notify_owned_proposal(*owned, group_id.clone())?;

        Ok((group_id.to_vec(), serialized_message, epoch))
    }

    pub fn remove_group_member(
        &self,
        group_number: usize,
        peer_id: PeerId,
    ) -> Result<(), MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec
            .get(group_number)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let group_store = map
            .get_mut(group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        let index = group_store
            .group
            .members()
            .position(|x| x.credential.serialized_content() == peer_id.to_bytes())
            .ok_or(NotAGroupMemberError { peer_id })?;
        if index == 0 {
            return Err(RemoveGroupOwnerError);
        }
        let mut res = Vec::new();

        if let Some(pending_commit) = group_store.group.pending_commit() {
            for proposal in pending_commit.add_proposals() {
                if let Some(peer_id) = group_store.added_pks.remove(
                    proposal
                        .add_proposal()
                        .key_package()
                        .leaf_node()
                        .signature_key(),
                ) {
                    res.push(peer_id);
                }
            }
        }

        let (mls_message_out, welcome_option, _) = group_store
            .group
            .remove_members(
                &self.provider,
                &self.signer,
                &[LeafNodeIndex::new(
                    index.try_into().map_err(|_| RemoveGroupMemberError)?,
                )],
            )
            .map_err(|_| RemoveGroupMemberError)?;
        let commit = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;
        group_store
            .group
            .merge_pending_commit(&self.provider)
            .map_err(|e| CreateGroupError {
                reason: e.to_string(),
            })?;
        group_store.update_cipher(&self.provider)?;

        let epoch = group_store.group.epoch().as_u64();

        let welcome = welcome_option
            .map(|welcome_out| {
                welcome_out
                    .tls_serialize_detached()
                    .map_err(|e| SerializationError(e.to_string()))
            })
            .transpose()?;

        self.instruction_tx
            .send(Instruction::Commit {
                group_id: group_id.clone(),
                peer_ids: res,
                commit,
                welcome,
                epoch,
            })
            .map_err(|e| CommitError {
                group_id: group_id.clone(),
                reason: e.to_string(),
            })
    }

    pub fn propose_remove_group_member(
        &self,
        group_number: usize,
        peer_id: PeerId,
    ) -> Result<(Vec<u8>, Vec<u8>, u64), MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec
            .get(group_number)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage { group, owned, .. } = map
            .get_mut(group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        let index = group
            .members()
            .position(|x| x.credential.serialized_content() == peer_id.to_bytes())
            .ok_or(NotAGroupMemberError { peer_id })?;
        if index == 0 {
            return Err(RemoveGroupOwnerError);
        }
        let (mls_message_out, _) = group
            .propose_remove_member(
                &self.provider,
                &self.signer,
                LeafNodeIndex::new(index.try_into().map_err(|_| RemoveGroupMemberError)?),
            )
            .map_err(|_| RemoveGroupMemberError)?;
        let serialized_message = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;
        self.notify_owned_proposal(*owned, group_id.clone())?;

        let epoch = group.epoch().as_u64();

        Ok((group_id.to_vec(), serialized_message, epoch))
    }

    pub fn update_key_material(
        &self,
        group_number: usize,
    ) -> Result<(Vec<u8>, Vec<u8>, u64), MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec
            .get(group_number)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage { group, owned, .. } = map
            .get_mut(group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        let (mls_message_out, _) = group
            .propose_self_update(&self.provider, &self.signer, LeafNodeParameters::default())
            .map_err(|_| GroupUpdateError)?;
        let serialized_message = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;
        self.notify_owned_proposal(*owned, group_id.clone())?;

        let epoch = group.epoch().as_u64();
        Ok((group_id.to_vec(), serialized_message, epoch))
    }

    pub fn delete_group_for_all(&self, group_number: usize) -> Result<Vec<u8>, MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec
            .get(group_number)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage { group, owned, .. } = map
            .get_mut(group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        if !*owned {
            return Err(NotGroupOwnerError);
        }

        let other_peers: Vec<LeafNodeIndex> = group.members().skip(1).map(|x| x.index).collect();
        //Return early if group already empty
        if other_peers.is_empty() {
            return Ok(group_id.to_vec());
        }
        // Clear Proposal Store
        group
            .commit_to_pending_proposals(&self.provider, &self.signer)
            .map_err(|e| DeleteAllError {
                reason: e.to_string(),
            })?;
        group.clear_pending_commit(self.provider.storage())?;
        let (mls_message_out, _, _) = group
            .remove_members(&self.provider, &self.signer, &other_peers)
            .map_err(|_| RemoveGroupMemberError)?;

        let commit = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;

        let epoch = group.epoch().as_u64();

        self.instruction_tx
            .send(Instruction::Commit {
                group_id: group_id.clone(),
                peer_ids: Vec::new(),
                commit,
                welcome: None,
                epoch,
            })
            .map_err(|e| CommitError {
                group_id: group_id.clone(),
                reason: e.to_string(),
            })?;

        Ok(group_id.to_vec())
    }

    pub fn store_proposal(
        &self,
        group_id_vec: Vec<u8>,
        proposal_vec: Vec<u8>,
    ) -> Result<(), MyOpenMLSError> {
        let group_id = GroupId::from_slice(&group_id_vec);
        // Taken before the group store is locked, since both read it.
        let common = self.get_common_group_id().ok();
        let admitted = self.common_group_peers();
        let mut map = self.group_store.lock().unwrap();
        let GroupStorage { group, owned, .. } = map
            .get_mut(&group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;

        let mls_message_in = MlsMessageIn::tls_deserialize_exact(&proposal_vec)
            .map_err(|e| SerializationError(e.to_string()))?;
        let priv_message_in = match mls_message_in.extract() {
            MlsMessageBodyIn::PrivateMessage(priv_msg) => priv_msg,
            _ => return Err(InvalidPrivMessage("".to_string())),
        };
        let protocol_message = ProtocolMessage::from(priv_message_in);

        match group
            .process_message(&self.provider, protocol_message)?
            .into_content()
        {
            ProcessedMessageContent::ProposalMessage(staged_proposal) => {
                if let Proposal::Remove(prop) = (*staged_proposal.clone()).proposal() {
                    //Only add proposal, if not group Owner
                    if prop.removed().usize() == 0 {
                        return Ok(());
                    }
                }
                // Private groups only take members of the common group (see check_admitted).
                if let Proposal::Add(add) = (*staged_proposal.clone()).proposal() {
                    let content = add
                        .key_package()
                        .leaf_node()
                        .credential()
                        .serialized_content();
                    let peer = PeerId::from_bytes(content).ok();
                    let nested = common.as_ref().is_some_and(|c| *c != group_id);
                    if nested && !peer.is_some_and(|p| admitted.contains(&p)) {
                        log::warn!(
                            "Ignoring a proposal to add {peer:?}, which is not in the common group"
                        );
                        return Ok(());
                    }
                }
                group.store_pending_proposal(self.provider.storage(), *staged_proposal)?
            }
            _ => return Err(InvalidProposal("".to_string())),
        }
        if *owned {
            self.group_commit_tx
                .send(group_id.clone())
                .map_err(|e| CommitError {
                    group_id,
                    reason: e.to_string(),
                })?;
        }

        Ok(())
    }

    pub async fn commit(&self, group_id: GroupId) -> Result<(), MyOpenMLSError> {
        let mut res = Vec::new();

        //makes sure that map is not locked during sleep
        let (mls_message_out, welcome_option, epoch) = {
            let mut map = self.group_store.lock().unwrap();
            let group_store = map
                .get_mut(&group_id)
                .ok_or(GroupNotAvailable(hex::encode(group_id.to_vec())))?;
            let group = &mut group_store.group;
            let (mls_message_out, welcome_option, _) = group
                .commit_to_pending_proposals(&self.provider, &self.signer)
                .map_err(|e| CommitError {
                    group_id: group_id.clone(),
                    reason: e.to_string(),
                })?;

            if let Some(pending_commit) = group.pending_commit() {
                for proposal in pending_commit.add_proposals() {
                    if let Some(peer_id) = group_store.added_pks.remove(
                        proposal
                            .add_proposal()
                            .key_package()
                            .leaf_node()
                            .signature_key(),
                    ) {
                        res.push(peer_id);
                    }
                }
            }

            if let Err(e) = group.merge_pending_commit(&self.provider) {
                return Err(CommitError {
                    group_id: group_id.clone(),
                    reason: e.to_string(),
                });
            }
            let epoch = group.epoch().as_u64();
            group_store.update_cipher(&self.provider)?;
            (mls_message_out, welcome_option, epoch)
        };

        sleep(Duration::from_secs(NETWORK_DELAY)).await;

        let commit = mls_message_out
            .tls_serialize_detached()
            .map_err(|e| SerializationError(e.to_string()))?;

        let welcome = welcome_option
            .map(|welcome_out| {
                welcome_out
                    .tls_serialize_detached()
                    .map_err(|e| SerializationError(e.to_string()))
            })
            .transpose()?;

        self.instruction_tx
            .send(Instruction::Commit {
                group_id: group_id.clone(),
                peer_ids: res,
                commit,
                welcome,
                epoch,
            })
            .map_err(|e| CommitError {
                group_id,
                reason: e.to_string(),
            })
    }

    pub fn get_ratchet_tree(&self, group_id_vec: &[u8]) -> Result<Vec<u8>, MyOpenMLSError> {
        let group_id = GroupId::from_slice(group_id_vec);
        let mut map = self.group_store.lock().unwrap();
        let group_option = map.get_mut(&group_id);
        match group_option {
            Some(group_store) => Ok(bincode::serialize(&group_store.group.export_ratchet_tree())
                .map_err(|e| SerializationError(e.to_string()))?),
            None => Err(GroupNotAvailable(
                "Could not find group for ratchet tree export".to_string(),
            )),
        }
    }

    /*
    Merges own Group with Commit and Returns a Vec of Peers to send Welcome Message to
     */
    pub fn handle_commit(
        &self,
        group_id_vec: Vec<u8>,
        commit_vec: Vec<u8>,
    ) -> Result<(Vec<PeerId>, Vec<u8>, bool), MyOpenMLSError> {
        let mut res = Vec::new();
        let mls_message_in = MlsMessageIn::tls_deserialize_exact(&commit_vec)
            .map_err(|e| SerializationError(e.to_string()))?;

        let priv_message_in = match mls_message_in.extract() {
            MlsMessageBodyIn::PrivateMessage(priv_msg) => priv_msg,
            _ => {
                return Err(InvalidPrivMessage("".to_string()));
            }
        };
        let protocol_message = ProtocolMessage::from(priv_message_in);

        let group_id = GroupId::from_slice(&group_id_vec);
        let (ratchet, self_removed) = {
            let mut map = self.group_store.lock().unwrap();
            let group_store = map
                .get_mut(&group_id)
                .ok_or(GroupNotAvailable("".to_string()))?;

            let processed_message = group_store
                .group
                .process_message(&self.provider, protocol_message)
                .map_err(MyProcessMessageError)?;
            // Only handle the commit if it is from the group owner. According to the RFC this should be secure: https://datatracker.ietf.org/doc/html/rfc9420#name-authentication
            // The leaf not index is not fixed in the RFC. In this OpenMls version it does not change
            if !matches!(processed_message.sender(), Member(node_index) if node_index.usize() == 0)
            {
                return Err(NonGroupOwnerCommitError {
                    sender: processed_message.sender().clone(),
                });
            }

            match processed_message.into_content() {
                ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                    //If I added a peer make sure to send him the Welcome Message
                    for proposal in staged_commit.add_proposals() {
                        if let Some(peer_id) = group_store.added_pks.remove(
                            proposal
                                .add_proposal()
                                .key_package()
                                .leaf_node()
                                .signature_key(),
                        ) {
                            res.push(peer_id)
                        }
                    }
                    let self_removed = staged_commit.self_removed();
                    group_store
                        .group
                        .merge_staged_commit(&self.provider, *staged_commit)
                        .map_err(|e| CommitError {
                            group_id: group_id.clone(),
                            reason: e.to_string(),
                        })?;
                    let res_tree = bincode::serialize(&group_store.group.export_ratchet_tree())
                        .map_err(|e| SerializationError(e.to_string()))?;
                    if !self_removed {
                        group_store.update_cipher(&self.provider)?;
                    }
                    (res_tree, self_removed)
                }
                _ => {
                    return Err(InvalidProposal("".to_string()));
                }
            }
        };

        Ok((res, ratchet, self_removed))
    }

    /// Checks a KeyPackage that `peer_id` sent: both its signatures, and that it belongs to
    /// `peer_id` (key_package_belongs_to). Returns it as validated by OpenMLS.
    pub fn validate_key_package(
        &self,
        key_package: KeyPackage,
        peer_id: PeerId,
    ) -> Result<KeyPackage, MyOpenMLSError> {
        let key_package = KeyPackageIn::from(key_package)
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| InvalidKeyPackage(e.to_string()))?;
        if !key_package_belongs_to(&key_package, peer_id) {
            return Err(InvalidKeyPackage(format!(
                "it does not belong to {peer_id}"
            )));
        }
        Ok(key_package)
    }

    //Stores the KeyPackage `key` sent, once validated, for further use
    pub fn insert_foreign_kp(&self, key: PeerId, value: KeyPackage) -> Result<(), MyOpenMLSError> {
        let value = self.validate_key_package(value, key)?;
        let mut map = self.key_package_store.lock().unwrap();
        map.insert(key, value);
        Ok(())
    }

    //Returns a list of all Groups
    pub fn get_groups(&self) -> Vec<GroupId> {
        let vec = self.group_ids.lock().unwrap();
        vec.clone()
    }

    //Returns the epoch of a group
    pub fn get_group_epoch(&self, group_id: &GroupId) -> Option<u64> {
        let map = self.group_store.lock().unwrap();
        map.get(group_id).map(|x| x.group.epoch().as_u64())
    }

    pub fn sign<T>(&self, body: T) -> Result<T::Output, MyOpenMLSError>
    where
        T: shared_structs::Signable + serde::Serialize,
    {
        let (grpkey, memkey) = {
            let key_store = self.key_store.lock().unwrap();
            (
                key_store.get_grpkey().ok_or(GPKUndefinedError)?,
                key_store.get_memkey().ok_or(GSKUndefinedError)?,
            )
        };
        let signature = thressig::sign(&memkey, &grpkey, &body)?;
        Ok(T::new(body, signature))
    }

    pub fn verify<T>(&self, message: T) -> Result<(T::Body, Signature), MyOpenMLSError>
    where
        T: shared_structs::Verifiable,
        <T as shared_structs::Verifiable>::Body: serde::Serialize,
    {
        let grpkey = self
            .key_store
            .lock()
            .unwrap()
            .get_grpkey()
            .ok_or(GPKUndefinedError)?;
        let (body, signature) = message.into_parts();
        let signature = signature.ok_or(SignatureMissing)?;
        if thressig::verify(&signature, &grpkey, &body).is_err() {
            return Err(SignatureInvalid {
                signature: Box::new(signature),
            });
        }
        Ok((body, signature))
    }

    /// Verifies the anonymous group signature of a gossip message. Relays call this before
    /// forwarding; an attribution, if any, is checked after decryption (open_broadcast, open_group).
    pub fn verify_message(
        &self,
        message: shared_structs::Message,
    ) -> Result<(shared_structs::MessageBody, Signature), MyOpenMLSError> {
        self.verify(message)
    }

    /// Attributes a message to this member: an attributed signature over the message's context
    /// (channel or group ID), topic and data, for the plaintext of the message.
    fn attribute(
        &self,
        context: &[u8],
        topic: &str,
        data: &[u8],
    ) -> Result<shared_structs::Attribution, MyOpenMLSError> {
        let (grpkey, memkey, member_id) = {
            let key_store = self.key_store.lock().unwrap();
            (
                key_store.get_grpkey().ok_or(GPKUndefinedError)?,
                key_store.get_memkey().ok_or(GSKUndefinedError)?,
                key_store.get_id().ok_or(IDUndefinedError)?,
            )
        };
        let signature = thressig::sign_attributed(
            &memkey,
            &grpkey,
            &attributed_content(context, topic, data, member_id),
        )?;
        Ok(shared_structs::Attribution {
            member_id,
            signature,
        })
    }

    /// Checks an attribution found inside a decrypted message and returns its MemberID.
    /// An attribution that does not verify, or was made for another channel or group, is
    /// rejected, so a MemberID can be neither forged nor moved to another message.
    fn check_attribution(
        &self,
        context: &[u8],
        topic: &str,
        data: &[u8],
        attribution: Option<shared_structs::Attribution>,
    ) -> Result<Option<usize>, MyOpenMLSError> {
        let Some(shared_structs::Attribution {
            member_id,
            signature,
        }) = attribution
        else {
            return Ok(None);
        };
        let grpkey = self
            .key_store
            .lock()
            .unwrap()
            .get_grpkey()
            .ok_or(GPKUndefinedError)?;
        let content = attributed_content(context, topic, data, member_id);
        if thressig::verify(&signature, &grpkey, &content).is_ok()
            && thressig::signed_by(&signature, &grpkey, member_id)
        {
            Ok(Some(member_id))
        } else {
            Err(AttributionInvalid { member_id })
        }
    }

    pub fn verify_enc_share(
        &self,
        enc_share: &EncShare,
        commits: &[thressig::G1],
        elgamal_pk: Option<thressig::G2>,
    ) -> bool {
        let (group_key, elgamal_pk) = {
            let keystore = self.key_store.lock().unwrap();
            let group_key = match keystore.get_grpkey() {
                Some(v) => v,
                None => return false,
            };

            let elgamal_pk = elgamal_pk.or_else(|| keystore.get_elgamal_pubkey().cloned());
            let elgamal_pk = match elgamal_pk {
                Some(v) => v,
                None => return false,
            };

            (group_key, elgamal_pk)
        };
        thressig::verify_enc_share_correctness(enc_share, &group_key, &elgamal_pk, commits)
    }

    /// The members in `gml` that made `signature`, from the archives' partial openings.
    pub fn open(
        &self,
        signature: &Signature,
        gml: HashSet<usize>,
        all_sj: HashMap<usize, Vec<(usize, thressig::GT)>>,
    ) -> Vec<usize> {
        let group_key = match self.key_store.lock().unwrap().get_grpkey() {
            Some(v) => v,
            None => return Vec::new(),
        };

        thressig::open(signature, &group_key, &gml, &all_sj)
    }

    /// The sharing peers of `member`, which `open` found to have made `signature`.
    pub fn open_peers_of<K>(
        &self,
        signature: &Signature,
        member: usize,
        peers: HashSet<K>,
        all_sj: HashMap<usize, Vec<(K, thressig::GT)>>,
    ) -> Vec<K>
    where
        K: Eq + Clone + std::hash::Hash,
    {
        let group_key = match self.key_store.lock().unwrap().get_grpkey() {
            Some(v) => v,
            None => return Vec::new(),
        };

        thressig::open_peers_of(signature, &group_key, member, &peers, &all_sj)
    }

    /*Encrypts a message for specific group
    Returns: GroupId + Ciphertext + Nonce*/
    pub fn encrypt_with_group_key(
        &self,
        group_id: GroupId,
        plaintext: Vec<u8>,
    ) -> Result<EncryptedGroupKey, MyOpenMLSError> {
        let to_encrypt =
            bincode::serialize(&plaintext).map_err(|e| SerializationError(e.to_string()))?;

        let map = self.group_store.lock().unwrap();
        let GroupStorage { aes_cipher, .. } = map
            .get(&group_id)
            .ok_or(GroupNotAvailable("".to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = aes_cipher.encrypt(&nonce, to_encrypt.as_ref()).unwrap();
        Ok((group_id.to_vec(), ciphertext, nonce.to_vec()))
    }

    //Decrypts a message for specific group
    //Returns Group Number Option, because Common Group does not have a number
    pub fn decrypt_with_group_key(
        &self,
        group_id_vec: Vec<u8>,
        ciphertext: Vec<u8>,
        ser_nonce: Vec<u8>,
    ) -> Result<(Option<usize>, Vec<u8>), MyOpenMLSError> {
        // Nonce and ciphertext arrive from the network: a malformed or undecryptable
        // message must be rejected, not crash the node (from_slice panics on a bad length).
        if ser_nonce.len() != 12 {
            return Err(DecryptionError);
        }
        let nonce = GenericArray::from_slice(&ser_nonce[..]);
        let group_id = GroupId::from_slice(&group_id_vec);
        let group_number = {
            let vec = self.group_ids.lock().unwrap();
            vec.iter().position(|r| *r == group_id)
        };

        let map = self.group_store.lock().unwrap();
        let GroupStorage { aes_cipher, .. } = map
            .get(&group_id)
            .ok_or(GroupNotAvailable(hex::encode(group_id.to_vec())))?;
        let plaintext: Vec<u8> = bincode::deserialize(
            &aes_cipher
                .decrypt(nonce, ciphertext.as_ref())
                .map_err(|_| DecryptionError)?,
        )
        .map_err(|e| SerializationError(e.to_string()))?;
        Ok((group_number, plaintext))
    }

    //Decrypts a message which arrives for the broadcast group
    pub fn decrypt_broadcast(
        &self,
        ciphertext: Vec<u8>,
        ser_nonce: Vec<u8>,
    ) -> Result<Vec<u8>, MyOpenMLSError> {
        let (_, plain) = self.decrypt_with_group_key(
            self.get_common_group_id()?.to_vec(),
            ciphertext,
            ser_nonce,
        )?;
        Ok(plain)
    }

    // For a list of Peerids returns all available and missing keypackets, if missing is empty remove all keypackages from storage
    pub fn get_keypackages_for_peer_ids(
        &self,
        peer_ids: &Vec<PeerId>,
    ) -> Result<Vec<(PeerId, KeyPackage)>, MyOpenMLSError> {
        let mut map = self.key_package_store.lock().unwrap();
        let mut res = Vec::new();
        let mut missing = Vec::new();
        for peer_id in peer_ids {
            let keypackage = map.get(peer_id);
            if let Some(key) = keypackage {
                res.push((*peer_id, key.clone()));
            } else {
                missing.push(*peer_id)
            }
        }
        if !missing.is_empty() {
            return Err(MissingKeyPackagesError { peer_ids: missing });
        }
        for peer_id in peer_ids {
            map.remove(peer_id);
        }
        Ok(res)
    }

    // For a Peerids returns a keypackage
    pub fn get_keypackage(&self, peer_id: PeerId) -> Option<KeyPackage> {
        let mut map = self.key_package_store.lock().unwrap();
        map.remove(&peer_id)
    }

    // A helper to create key package bundles.
    pub fn generate_key_package(&self) -> Result<KeyPackageBundle, MyOpenMLSError> {
        // Create the key package
        KeyPackage::builder()
            .build(
                self.ciphersuite,
                &self.provider,
                &self.signer,
                self.credential_with_key.clone(),
            )
            .map_err(|_| NewKeyPackageError)
    }

    pub fn subscibed_to_topic(&self, topic: Vec<u8>) -> bool {
        let map = self.subscribed_topics.lock().unwrap();
        let channel = self.get_channel(topic.clone());
        map.get(&channel).is_some_and(|x| x.contains(&topic))
    }

    pub fn get_channel(&self, topic: Vec<u8>) -> String {
        channel_of(&topic)
    }

    /// Encrypts a broadcast for the common group, with the topic inside the
    /// ciphertext and only its channel in clear.
    /// With `attributed`, the plaintext also carries this member's attribution (see attribute).
    pub fn seal_broadcast(
        &self,
        topic: &str,
        data: Vec<u8>,
        attributed: bool,
    ) -> Result<shared_structs::MessageBody, MyOpenMLSError> {
        let channel = channel_of(topic.as_bytes());
        let attribution = match attributed {
            true => Some(self.attribute(channel.as_bytes(), topic, &data)?),
            false => None,
        };
        let (_, ciphertext, nonce) = self.encrypt_with_group_key(
            self.get_common_group_id()?,
            encode_broadcast(topic, data, attribution)?,
        )?;
        Ok(shared_structs::MessageBody::BroadcastEnc {
            channel,
            data: ciphertext,
            nonce,
        })
    }

    /// Decrypts a broadcast that arrived on `channel` and returns its topic, data and, for an
    /// attributed message, the verified MemberID. Fails if the topic inside does not belong to
    /// that channel, or if the attribution does not verify.
    pub fn open_broadcast(
        &self,
        channel: &str,
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
    ) -> Result<(String, Vec<u8>, Option<usize>), MyOpenMLSError> {
        let (topic, data, attribution) =
            decode_broadcast(channel, &self.decrypt_broadcast(ciphertext, nonce)?)?;
        let member = self.check_attribution(channel.as_bytes(), &topic, &data, attribution)?;
        Ok((topic, data, member))
    }

    /// Encrypts a message for a private group, nested inside the common group: the private
    /// group's key encrypts the data, and the common group's current key encrypts the result.
    /// A peer that the AA removed from the common group lacks the common group's new key, so
    /// it can neither send nor read private-group messages, even while it is still a member of
    /// the private group.
    /// With `attributed`, the innermost plaintext also carries this member's attribution, so
    /// only members of the private group learn the MemberID.
    pub fn seal_group(
        &self,
        group_number: usize,
        data: Vec<u8>,
        attributed: bool,
    ) -> Result<shared_structs::MessageBody, MyOpenMLSError> {
        let group_id = self.get_group_id(group_number)?;
        let attribution = match attributed {
            true => Some(self.attribute(group_id.as_slice(), "", &data)?),
            false => None,
        };
        let plain = bincode::serialize(&shared_structs::GroupPlain { data, attribution })
            .map_err(|e| SerializationError(e.to_string()))?;
        let (group_id, inner, inner_nonce) = self.encrypt_with_group_key(group_id, plain)?;
        let layer = bincode::serialize(&(inner, inner_nonce))
            .map_err(|e| SerializationError(e.to_string()))?;
        let (_, enc_data, nonce) =
            self.encrypt_with_group_key(self.get_common_group_id()?, layer)?;
        Ok(shared_structs::MessageBody::GroupEnc {
            group_id,
            enc_data,
            nonce,
        })
    }

    /// Removes the common group's layer of a private-group message and returns the private
    /// group's ciphertext and nonce. Archives store these, since they cannot decrypt further.
    pub fn unwrap_group(
        &self,
        enc_data: Vec<u8>,
        nonce: Vec<u8>,
    ) -> Result<(Vec<u8>, Vec<u8>), MyOpenMLSError> {
        let (_, layer) = self
            .decrypt_with_group_key(self.get_common_group_id()?.to_vec(), enc_data, nonce)
            .map_err(|e| match e {
                DecryptionError => CommonLayerError,
                e => e,
            })?;
        bincode::deserialize(&layer).map_err(|e| SerializationError(e.to_string()))
    }

    /// Decrypts a private-group message: first the common group's layer, then the private
    /// group's. Returns the group number, the data and, for an attributed message, the verified
    /// MemberID; GroupNotAvailable if this peer is not a member.
    pub fn open_group(
        &self,
        group_id: Vec<u8>,
        enc_data: Vec<u8>,
        nonce: Vec<u8>,
    ) -> Result<OpenedGroupMessage, MyOpenMLSError> {
        if !self
            .group_store
            .lock()
            .unwrap()
            .contains_key(&GroupId::from_slice(&group_id))
        {
            return Err(GroupNotAvailable(hex::encode(&group_id)));
        }
        let (inner, inner_nonce) = self.unwrap_group(enc_data, nonce)?;
        let (group_number, plain) =
            self.decrypt_with_group_key(group_id.clone(), inner, inner_nonce)?;
        let shared_structs::GroupPlain { data, attribution } =
            bincode::deserialize(&plain).map_err(|e| SerializationError(e.to_string()))?;
        let member = self.check_attribution(&group_id, "", &data, attribution)?;
        Ok((group_number, data, member))
    }

    //Returns Channel, if not already subscribed
    pub fn subscribe(&self, topic: Vec<u8>) -> Result<Option<String>, MyOpenMLSError> {
        let mut map = self.subscribed_topics.lock().unwrap();
        let channel = self.get_channel(topic.clone());
        match map.get_mut(&channel) {
            Some(val) => {
                if val.contains(&topic) {
                    Err(AlreadySubscribedError {
                        topic: topic.clone(),
                    })
                } else {
                    val.push(topic);
                    Ok(None)
                }
            }
            None => {
                map.insert(channel.clone(), vec![topic]);
                Ok(Some(channel))
            }
        }
    }

    //Returns Channel, if it has no subscribed topics and gossipsub can unsubscribe from it.
    pub fn unsubscribe(&self, topic: Vec<u8>) -> Result<Option<String>, MyOpenMLSError> {
        let mut map = self.subscribed_topics.lock().unwrap();
        let channel = self.get_channel(topic.clone());
        let channel_list = map.get_mut(&channel).ok_or(NoSubscriptionError {
            topic: topic.clone(),
        })?;
        {
            if !channel_list.contains(&topic) {
                Err(NoSubscriptionError { topic })
            } else {
                channel_list.retain(|x| *x != topic);
                if channel_list.is_empty() {
                    map.remove(&channel);
                    Ok(Some(channel))
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub fn get_enc_share_pairings<K>(
        &self,
        enc_shares: Vec<(K, Vec<u8>)>,
        signature: &[u8],
    ) -> Result<Vec<(K, EncSharePairing)>, MyOpenMLSError>
    where
        K: Clone,
    {
        let keystore = self.key_store.lock().unwrap();
        let secret_key = match keystore.get_elgamal_secretkey() {
            Some(v) => v,
            None => return Err(ElgamalUndefinedError),
        };

        let signature: Signature =
            bincode::deserialize(signature).map_err(|e| SerializationError(e.to_string()))?;

        let enc_shares: Result<Vec<(K, EncSharePairing)>, bincode::Error> = enc_shares
            .iter()
            .map(|(id, enc_share)| {
                let enc_share: EncShare = bincode::deserialize(enc_share)?;
                let pairing = thressig::create_open_pairing(&signature, enc_share, secret_key);
                Ok((id.clone(), pairing))
            })
            .collect();

        enc_shares.map_err(|e| SerializationError(e.to_string()))
    }

    //Notifies the group owner, that there are new proposals
    fn notify_owned_proposal(&self, owned: bool, group_id: GroupId) -> Result<(), MyOpenMLSError> {
        if owned {
            self.group_commit_tx
                .send(group_id.clone())
                .map_err(|e| CommitError {
                    group_id: group_id.clone(),
                    reason: e.to_string(),
                })?
        }

        Ok(())
    }
}

fn generate_openmls_credentials(
    provider: &impl OpenMlsProvider,
    skp: SignatureKeyPair,
    peer_id: PeerId,
) -> Result<(CredentialWithKey, SignatureKeyPair), MyOpenMLSError> {
    let credential = BasicCredential::new(peer_id.to_bytes());
    // Store the signature key into the key store so OpenMLS has access
    // to it.
    skp.store(provider.storage())
        .map_err(|_| GenerateCredentialsError {
            reason: "Store to provider did not work".to_string(),
        })?;

    Ok((
        CredentialWithKey {
            credential: credential.into(),
            signature_key: skp.public().into(),
        },
        skp,
    ))
}

/// True if a KeyPackage names `peer_id` twice over: as its credential, by which members are
/// known (common_group_peers), and through its signature key, the Ed25519 key of the PeerID.
/// A KeyPackage that fails either would add another PeerID than the one that sent it.
pub fn key_package_belongs_to(key_package: &KeyPackage, peer_id: PeerId) -> bool {
    let leaf_node = key_package.leaf_node();
    let credential_matches = leaf_node.credential().serialized_content() == peer_id.to_bytes();
    let key_matches =
        libp2p::identity::ed25519::PublicKey::try_from_bytes(leaf_node.signature_key().as_slice())
            .is_ok_and(|key| libp2p::identity::PublicKey::from(key).to_peer_id() == peer_id);
    credential_matches && key_matches
}

struct GroupStorage {
    group: MlsGroup,
    aes_cipher: Aes256Gcm,
    owned: bool,
    added_pks: HashMap<SignaturePublicKey, PeerId>,
}

impl GroupStorage {
    fn update_cipher(&mut self, provider: &OpenMlsRustCrypto) -> Result<(), MyOpenMLSError> {
        let secret = self
            .group
            .export_secret(provider, "Test", "Test".as_bytes(), 32)?;
        let key = Key::<Aes256Gcm>::from_slice(&secret);
        let aes_cipher = Aes256Gcm::new(key);
        self.aes_cipher = aes_cipher;
        Ok(())
    }
}

/// The channel (GossipSub topic) a topic is published on.
pub fn channel_of(topic: &[u8]) -> String {
    // If topic is an ASCII number 0..9, map directly to that channel index.
    if let Ok(s) = std::str::from_utf8(topic) {
        if let Ok(n) = s.parse::<usize>() {
            if n < NUM_CHANNELS as usize {
                return format!("channel_{}", n);
            }
        }
    }
    // Otherwise hash to one of NUM_CHANNELS buckets.
    let mut hasher = DefaultHasher::new();
    topic.hash(&mut hasher);
    format!("channel_{}", hasher.finish() % NUM_CHANNELS)
}

/// What an attribution signs: the message's context (channel or group ID), topic (empty for
/// private groups), data and the MemberID, with a label that keeps it apart from all other
/// group signatures.
fn attributed_content<'a>(
    context: &'a [u8],
    topic: &'a str,
    data: &'a [u8],
    member_id: usize,
) -> (&'static str, &'a [u8], &'a str, &'a [u8], usize) {
    ("dactie/attribution/1", context, topic, data, member_id)
}

/// The plaintext of a broadcast: topic, data and an optional attribution together.
pub fn encode_broadcast(
    topic: &str,
    data: Vec<u8>,
    attribution: Option<shared_structs::Attribution>,
) -> Result<Vec<u8>, MyOpenMLSError> {
    bincode::serialize(&shared_structs::BroadcastPlain {
        topic: topic.to_owned(),
        data,
        attribution,
    })
    .map_err(|e| MyOpenMLSError::SerializationError(e.to_string()))
}

/// Unpacks a decrypted broadcast that arrived on `channel`. A topic that does
/// not belong to that channel is rejected: its sender bypassed `channel_of`.
pub fn decode_broadcast(
    channel: &str,
    plain: &[u8],
) -> Result<(String, Vec<u8>, Option<shared_structs::Attribution>), MyOpenMLSError> {
    let inner: shared_structs::BroadcastPlain = bincode::deserialize(plain)
        .map_err(|e| MyOpenMLSError::SerializationError(e.to_string()))?;
    if channel_of(inner.topic.as_bytes()) != channel {
        return Err(MyOpenMLSError::ChannelMismatch {
            topic: inner.topic,
            channel: channel.to_owned(),
        });
    }
    Ok((inner.topic, inner.data, inner.attribution))
}

#[derive(Debug, Error)]
pub enum MyOpenMLSError {
    #[error("Could not find group in storage")]
    GroupNotAvailable(String),
    #[error("The signature is invalid")]
    SignatureInvalid {
        signature: Box<thressig::structs::Signature>,
    },
    #[error("No signature available")]
    SignatureMissing,
    #[error("Could not add members to group {group_id:?}, because {reason:?}")]
    GroupAddMember { group_id: GroupId, reason: String },
    #[error("Deserialization Error, in Encryption")]
    SerializationError(String),
    #[error("Topic `{topic}` does not belong to channel `{channel}`")]
    ChannelMismatch { topic: String, channel: String },
    #[error("Not subscribed to topic: `{topic:?}`")]
    NoSubscriptionError { topic: Vec<u8> },
    #[error("Already subscribed to topic: `{topic:?}`")]
    AlreadySubscribedError { topic: Vec<u8> },
    #[error("Missing KeyPackages for the following Peers: `{peer_ids:?}`")]
    MissingKeyPackagesError { peer_ids: Vec<PeerId> },
    #[error("Did not receive a proposal")]
    InvalidProposal(String),
    #[error("Did not receive a private Message")]
    InvalidPrivMessage(String),
    #[error("Could not create add_member proposal")]
    MyProposeAddMemberError(#[from] ProposeAddMemberError<MemoryStorageError>),
    #[error("Could not parese Protocol Message")]
    MyProcessMessageError(#[from] ProcessMessageError<MemoryStorageError>),
    #[error("Memory Storage Error")]
    MyMemoryStorageeError(#[from] MemoryStorageError),
    #[error("Was not able to commit to group {group_id:?} because `{reason:?}`")]
    CommitError { group_id: GroupId, reason: String },
    #[error("Got a Commit from a not group owner {sender:?}")]
    NonGroupOwnerCommitError { sender: Sender },
    #[error("Peer {peer_id:?} is not a group member")]
    NotAGroupMemberError { peer_id: PeerId },
    #[error("Can not Remove Group Owner")]
    RemoveGroupOwnerError,
    #[error("Can not Remove Group Member")]
    RemoveGroupMemberError,
    #[error("Can not delete group for all, because not group owner")]
    NotGroupOwnerError,
    #[error("Could not Update MLS Group")]
    GroupUpdateError,
    #[error("Could not generate new Keypackage")]
    NewKeyPackageError,
    #[error("File IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Could not export Secret")]
    MLSExportSecretError(#[from] ExportSecretError<MemoryStorageError>),
    #[error("Could not create new MLS Group: `{reason:?}`")]
    CreateGroupError { reason: String },
    #[error("Could not store Group and topic information to file: `{reason:?}`")]
    StoreError { reason: String },
    #[error("Could not join Group from welcome: `{reason:?}`")]
    JoinGroupError { reason: String },
    #[error("Could not delete group for all: `{reason:?}`")]
    DeleteAllError { reason: String },
    #[error("Could not load Group Configuration from File: `{reason:?}`")]
    LoadError { reason: String },
    #[error("Could not generate Credentials: `{reason:?}`")]
    GenerateCredentialsError { reason: String },
    #[error("GroupMemberKey not set, can not connect do Dactie Network")]
    GSKUndefinedError,
    #[error("GroupPublicKey not set, can not connect do Dactie Network")]
    GPKUndefinedError,
    #[error("ID not set, can not go forward")]
    IDUndefinedError,
    #[error("Elgamalkeys not set")]
    ElgamalUndefinedError,
    #[error("Opening threshold not set")]
    ThresholdUndefinedError,
    #[error("Group Signature Operation failed: {0}")]
    ThressigError(#[from] thressig::ThressigError),
    #[error("CommonGroup Not Set")]
    CommonGroupUndefinedError,
    #[error(
        "Could not decrypt the group message (malformed, or encrypted under another epoch's key)"
    )]
    DecryptionError,
    #[error("The private-group message does not decrypt under the common group's current key: its sender is not admitted, or it was sent in another epoch")]
    CommonLayerError,
    #[error("{peer_id} is not in the common group, so it cannot be added to a private group")]
    NotAdmittedError { peer_id: PeerId },
    #[error("The path to the keystore is not valid or the file is corrupted")]
    KeyMaterialFileError,
    #[error("Invalid key material: {0}")]
    InvalidKeyMaterial(String),
    #[error("Could not write the key material file: {0}")]
    KeyMaterialWriteError(String),
    #[error("Invalid KeyPackage: {0}")]
    InvalidKeyPackage(String),
    #[error("The message's attribution to member {member_id} does not verify")]
    AttributionInvalid { member_id: usize },
}

#[cfg(test)]
mod broadcast_tests {
    use super::*;

    /// Topic and data come back out, and the channel is the topic's own.
    #[test]
    fn broadcast_plaintext_round_trips() {
        let channel = channel_of(b"Perforce Helix Core");
        let plain = encode_broadcast("Perforce Helix Core", b"IOC".to_vec(), None).unwrap();
        let (topic, data, attribution) = decode_broadcast(&channel, &plain).unwrap();
        assert_eq!(attribution, None);
        assert_eq!(topic, "Perforce Helix Core");
        assert_eq!(data, b"IOC");
    }

    /// A topic that does not belong to the channel it arrived on is rejected.
    #[test]
    fn topic_outside_its_channel_is_rejected() {
        let plain = encode_broadcast("3", b"x".to_vec(), None).unwrap();
        assert_eq!(channel_of(b"3"), "channel_3");
        assert!(matches!(
            decode_broadcast("channel_4", &plain),
            Err(MyOpenMLSError::ChannelMismatch { .. })
        ));
    }

    /// The channel mapping is unchanged: topics "0".."9" map directly, others by
    /// hash, as `get_channel` always did.
    #[test]
    fn channels_follow_the_existing_mapping() {
        for n in 0..NUM_CHANNELS {
            assert_eq!(channel_of(n.to_string().as_bytes()), format!("channel_{n}"));
        }
        let mut hasher = DefaultHasher::new();
        b"Perforce Helix Core".to_vec().hash(&mut hasher);
        assert_eq!(
            channel_of(b"Perforce Helix Core"),
            format!("channel_{}", hasher.finish() % NUM_CHANNELS)
        );
    }
}

#[cfg(test)]
mod remote_input_tests {
    use super::*;

    fn node(name: &str) -> MyOpenMls {
        let dir = std::env::temp_dir().join(format!("dactie-mls-{name}-{}", std::process::id()));
        let (group_commit_tx, _) = mpsc::unbounded_channel();
        let (instruction_tx, _) = mpsc::unbounded_channel();
        MyOpenMls::new(
            StoredKeyMaterial::new(None, None, None, None),
            group_commit_tx,
            instruction_tx,
            dir,
            PeerId::random(),
        )
        .unwrap()
    }

    /// A JoinGroup request carries whatever MLS message its sender put in it.
    #[test]
    fn join_group_without_a_welcome_is_refused() {
        let node = node("welcome");
        let key_package = node.generate_key_package().unwrap().key_package().clone();
        let not_a_welcome = MlsMessageOut::from(key_package)
            .tls_serialize_detached()
            .unwrap();
        assert!(matches!(
            node.add_group_from_welcome(not_a_welcome, Vec::new()),
            Err(JoinGroupError { .. })
        ));
        assert!(node
            .add_group_from_welcome(vec![1, 2, 3], Vec::new())
            .is_err());
    }

    /// A KeyPackage is used only for the PeerID that both its credential and its signature
    /// key name.
    #[test]
    fn key_package_must_belong_to_its_sender() {
        let node = node("keypackage");
        let key_package = node.generate_key_package().unwrap().key_package().clone();
        assert!(node
            .validate_key_package(key_package.clone(), node.get_peerid())
            .is_ok());
        assert!(matches!(
            node.insert_foreign_kp(PeerId::random(), key_package),
            Err(InvalidKeyPackage(_))
        ));

        // Signed with this node's key, but naming another PeerID as its credential.
        let victim = PeerId::random();
        let skp = node.key_store.lock().unwrap().get_skp();
        let (credential, signer) =
            generate_openmls_credentials(&node.provider, skp, victim).unwrap();
        let forged = KeyPackage::builder()
            .build(node.ciphersuite, &node.provider, &signer, credential)
            .unwrap()
            .key_package()
            .clone();
        assert!(!key_package_belongs_to(&forged, victim));
        assert!(!key_package_belongs_to(&forged, node.get_peerid()));
        assert!(node.validate_key_package(forged, victim).is_err());
    }

    /// A member key that arrives before this node started a join is refused.
    #[test]
    fn member_key_without_a_join_is_refused() {
        let node = node("memkey");
        assert!(matches!(
            node.add_partial_memkey(vec![0; 16]),
            Err(InvalidKeyMaterial(_))
        ));
    }
}

#[cfg(test)]
mod group_saves_tests {
    use super::*;

    fn node(
        dir: &std::path::Path,
        key_store: &StoredKeyMaterial,
    ) -> Result<MyOpenMls, MyOpenMLSError> {
        let (group_commit_tx, _) = mpsc::unbounded_channel();
        let (instruction_tx, _) = mpsc::unbounded_channel();
        MyOpenMls::new(
            key_store.clone(),
            group_commit_tx,
            instruction_tx,
            dir.to_path_buf(),
            PeerId::random(),
        )
    }

    /// Groups survive a restart, also from the two files an earlier version wrote, and a
    /// damaged save stops the node instead of starting it without its groups.
    #[test]
    fn groups_are_restored_or_the_node_refuses_to_start() {
        let dir = std::env::temp_dir().join(format!("dactie-group-saves-{}", std::process::id()));
        let saves = dir.join("group_saves");
        let key_store = StoredKeyMaterial::new(None, None, None, None);
        let first = node(&dir, &key_store).unwrap();
        let group_number = first.create_empty_mls_group().unwrap();
        first.move_to_common(group_number);
        first.store_to_file().unwrap();
        let common = first.get_common_group_id().unwrap();
        let restored = || {
            node(&dir, &key_store)
                .unwrap()
                .get_common_group_id()
                .unwrap()
        };
        assert_eq!(restored(), common);

        // The former layout: groups in mstore, the key store in provider.
        let (groups, _): (Vec<u8>, HashMap<Vec<u8>, Vec<u8>>) =
            bincode::deserialize(&fs::read(saves.join(GROUP_STATE)).unwrap()).unwrap();
        fs::write(saves.join(LEGACY_GROUPS), groups).unwrap();
        let provider = fs::File::create(saves.join(LEGACY_PROVIDER)).unwrap();
        first.provider.storage().save_to_file(&provider).unwrap();
        fs::remove_file(saves.join(GROUP_STATE)).unwrap();
        assert_eq!(restored(), common);

        // A save replaces the former files.
        first.store_to_file().unwrap();
        assert!(!saves.join(LEGACY_GROUPS).exists());

        fs::write(saves.join(GROUP_STATE), b"truncated").unwrap();
        assert!(node(&dir, &key_store).is_err());
        fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod nested_group_tests {
    use super::*;
    use crate::shared_structs::MessageBody;

    struct Node {
        mls: MyOpenMls,
        instructions: mpsc::UnboundedReceiver<Instruction>,
        _commits: mpsc::UnboundedReceiver<GroupId>,
    }

    fn node(name: &str) -> Node {
        node_with(name, StoredKeyMaterial::new(None, None, None, None))
    }

    /// A group key and the member keys of members 1 and 2, issued by the AA as in a join
    /// with two archives and threshold 2.
    fn member_keys() -> (
        thressig::structs::Grpkey,
        thressig::structs::Memkey,
        thressig::structs::Memkey,
    ) {
        let (grpkey, mgrkey) = thressig::setup();
        let pks: Vec<_> = (0..2)
            .map(|_| thressig::elgamal_keygen(&grpkey.g_tilde).1)
            .collect();
        let mut gml = HashMap::new();
        let mut issue = |id: usize| {
            let n = thressig::join_mgr1();
            let (g_sk, h_sk, pi1, pi2, pi3, sk, enc_shares, commits) =
                thressig::join_mem1(id, n.clone(), &grpkey, 2, &pks);
            let partial = thressig::join_mgr2(
                id,
                thressig::JoinMgr2Args {
                    gml: &mut gml,
                    name: format!("member {id}"),
                    mgrkey: &mgrkey,
                    grpkey: &grpkey,
                    n: &n,
                    g_sk: &g_sk,
                    h_sk: &h_sk,
                    opener_pubkeys: &pks,
                    commits: &commits,
                    pi1,
                    pi2,
                    pi3,
                    enc_shares: &enc_shares,
                },
            )
            .unwrap();
            thressig::join_mem2(&sk, &partial)
        };
        let (x, y) = (issue(1), issue(2));
        (grpkey, x, y)
    }

    fn node_with(name: &str, key_store: StoredKeyMaterial) -> Node {
        let dir = std::env::temp_dir().join(format!("dactie-nested-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (commit_tx, commit_rx) = mpsc::unbounded_channel();
        let (instruction_tx, instruction_rx) = mpsc::unbounded_channel();
        let mls =
            MyOpenMls::new(key_store, commit_tx, instruction_tx, dir, PeerId::random()).unwrap();
        Node {
            mls,
            instructions: instruction_rx,
            _commits: commit_rx,
        }
    }

    fn id(n: &Node) -> PeerId {
        n.mls.get_peerid()
    }

    fn key_package(n: &Node) -> KeyPackage {
        n.mls.generate_key_package().unwrap().key_package().clone()
    }

    fn number(n: &Node, group: &GroupId) -> usize {
        n.mls.get_groups().iter().position(|g| g == group).unwrap()
    }

    fn send(n: &Node, group: &GroupId, text: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        seal(n, group, text, false)
    }

    fn seal(
        n: &Node,
        group: &GroupId,
        text: &[u8],
        attributed: bool,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        match n
            .mls
            .seal_group(number(n, group), text.to_vec(), attributed)
            .unwrap()
        {
            MessageBody::GroupEnc {
                group_id,
                enc_data,
                nonce,
            } => (group_id, enc_data, nonce),
            _ => unreachable!(),
        }
    }

    /// The AA with a common group of x (member 1) and y (member 2), and a private group that x
    /// owns, with y. x and y hold member keys, so they can sign attributions.
    fn setup() -> (Node, Node, Node, GroupId, GroupId) {
        let (grpkey, x_key, y_key) = member_keys();
        let aa = node_with(
            "aa",
            StoredKeyMaterial::new(Some(grpkey.clone()), None, None, None),
        );
        let mut x = node_with(
            "x",
            StoredKeyMaterial::new(Some(grpkey.clone()), Some(x_key), None, Some(1)),
        );
        let y = node_with(
            "y",
            StoredKeyMaterial::new(Some(grpkey), Some(y_key), None, Some(2)),
        );
        let number = aa.mls.create_empty_mls_group().unwrap();
        aa.mls.move_to_common(number);
        let common = aa.mls.get_common_group_id().unwrap();
        let members = [(id(&x), key_package(&x)), (id(&y), key_package(&y))];
        let (_, _, welcome, _) = aa.mls.add_group_members(common.clone(), &members).unwrap();
        let tree = aa.mls.get_ratchet_tree(common.as_slice()).unwrap();
        for n in [&x, &y] {
            let group = n
                .mls
                .add_group_from_welcome(welcome.clone().unwrap(), tree.clone())
                .unwrap();
            n.mls.move_to_common_id(group).unwrap();
        }

        x.mls.insert_foreign_kp(id(&y), key_package(&y)).unwrap();
        let number = x.mls.create_mls_group(&vec![id(&y)]).unwrap();
        let private = x.mls.get_group_id(number).unwrap();
        let Ok(Instruction::Commit {
            welcome: Some(welcome),
            ..
        }) = x.instructions.try_recv()
        else {
            panic!("create_mls_group sends a commit with a welcome");
        };
        let tree = x.mls.get_ratchet_tree(private.as_slice()).unwrap();
        y.mls.add_group_from_welcome(welcome, tree).unwrap();
        (aa, x, y, common, private)
    }

    /// Once the AA removed y from the common group, y can neither send to nor read the private
    /// group, although it is still a member of it.
    #[test]
    fn private_group_messages_need_the_common_group_key() {
        let (aa, x, y, common, private) = setup();
        let (g, d, n) = send(&y, &private, b"ioc");
        assert_eq!(
            x.mls.open_group(g, d, n).unwrap(),
            (Some(number(&x, &private)), b"ioc".to_vec(), None)
        );

        let (commit, _) = aa
            .mls
            .remove_group_members(common.clone(), &[id(&y)])
            .unwrap();
        x.mls.handle_commit(common.to_vec(), commit).unwrap();

        let (g, d, n) = send(&y, &private, b"after removal");
        assert!(matches!(x.mls.open_group(g, d, n), Err(CommonLayerError)));
        let (g, d, n) = send(&x, &private, b"members only");
        assert!(matches!(y.mls.open_group(g, d, n), Err(CommonLayerError)));
    }

    /// Members of the common group outside the private group, such as archives, remove the
    /// outer layer but cannot read the message.
    #[test]
    fn common_members_unwrap_but_cannot_read() {
        let (aa, x, _y, _common, private) = setup();
        let (g, d, n) = send(&x, &private, b"ioc");
        let (inner, _) = aa.mls.unwrap_group(d.clone(), n.clone()).unwrap();
        assert_ne!(inner, b"ioc".to_vec());
        assert!(matches!(
            aa.mls.open_group(g, d, n),
            Err(GroupNotAvailable(_))
        ));
    }

    /// A peer outside the common group cannot be added to a private group.
    #[test]
    fn only_admitted_peers_join_private_groups() {
        let (_aa, x, _y, _common, _private) = setup();
        let z = node("z");
        x.mls.insert_foreign_kp(id(&z), key_package(&z)).unwrap();
        let groups = x.mls.get_groups().len();
        assert!(matches!(
            x.mls.create_mls_group(&vec![id(&z)]),
            Err(NotAdmittedError { peer_id }) if peer_id == id(&z)
        ));
        assert_eq!(
            x.mls.get_groups().len(),
            groups,
            "no empty group is left behind"
        );
    }

    /// The owner removes a peer that left the common group from its private groups.
    #[test]
    fn owner_removes_peers_that_left_the_common_group() {
        let (aa, mut x, y, common, private) = setup();
        let (commit, _) = aa
            .mls
            .remove_group_members(common.clone(), &[id(&y)])
            .unwrap();
        x.mls.handle_commit(common.to_vec(), commit).unwrap();

        let removed = x
            .mls
            .remove_from_owned_groups(&HashSet::from([id(&y)]))
            .unwrap();
        assert_eq!(removed, vec![(number(&x, &private), id(&y))]);
        let members = x.mls.group_store.lock().unwrap()[&private]
            .group
            .members()
            .count();
        assert_eq!(members, 1);
        let Ok(Instruction::Commit {
            group_id, commit, ..
        }) = x.instructions.try_recv()
        else {
            panic!("the removal is published as a commit");
        };
        assert_eq!(group_id, private);
        let (_, _, self_removed) = y.mls.handle_commit(private.to_vec(), commit).unwrap();
        assert!(self_removed);
    }

    /// An attributed broadcast names its member only inside the ciphertext: the gossip message
    /// carries an anonymous signature, and a reader of the common group gets the MemberID.
    #[test]
    fn attributed_broadcast_names_member_inside_ciphertext() {
        let (aa, x, y, _common, _private) = setup();
        let body = x
            .mls
            .seal_broadcast("firewall", b"ioc".to_vec(), true)
            .unwrap();
        let message = x.mls.sign(body).unwrap();
        let grpkey = aa.mls.key_store.lock().unwrap().get_grpkey().unwrap();
        assert!(
            !thressig::signed_by(&message.signature, &grpkey, 1),
            "outer signature is anonymous"
        );
        let (body, _) = y.mls.verify_message(message).unwrap();
        let MessageBody::BroadcastEnc {
            channel,
            data,
            nonce,
        } = body
        else {
            unreachable!()
        };
        let opened = y.mls.open_broadcast(&channel, data, nonce).unwrap();
        assert_eq!(opened, ("firewall".to_string(), b"ioc".to_vec(), Some(1)));

        let MessageBody::BroadcastEnc {
            channel,
            data,
            nonce,
        } = x
            .mls
            .seal_broadcast("firewall", b"ioc".to_vec(), false)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(y.mls.open_broadcast(&channel, data, nonce).unwrap().2, None);
    }

    /// An attribution cannot be moved to other data or claimed for another member.
    #[test]
    fn forged_attribution_is_rejected() {
        let (_aa, x, y, common, _private) = setup();
        let channel = channel_of(b"firewall");
        let real = x
            .mls
            .attribute(channel.as_bytes(), "firewall", b"ioc")
            .unwrap();
        let open = |attribution: shared_structs::Attribution, data: &[u8]| {
            let plain = encode_broadcast("firewall", data.to_vec(), Some(attribution)).unwrap();
            let (_, ciphertext, nonce) =
                x.mls.encrypt_with_group_key(common.clone(), plain).unwrap();
            y.mls.open_broadcast(&channel, ciphertext, nonce)
        };
        assert_eq!(open(real.clone(), b"ioc").unwrap().2, Some(1));
        assert!(matches!(
            open(real.clone(), b"other ioc"),
            Err(AttributionInvalid { member_id: 1 })
        ));
        let claimed = shared_structs::Attribution {
            member_id: 2,
            ..real
        };
        assert!(matches!(
            open(claimed, b"ioc"),
            Err(AttributionInvalid { member_id: 2 })
        ));
    }

    /// In a private group, the MemberID is inside both layers: group members read it, while
    /// common-group members outside the group, such as archives, cannot.
    #[test]
    fn attributed_group_message_names_member_to_group_only() {
        let (aa, x, y, _common, private) = setup();
        let (g, d, n) = seal(&x, &private, b"ioc", true);
        assert_eq!(
            y.mls.open_group(g.clone(), d.clone(), n.clone()).unwrap(),
            (Some(number(&y, &private)), b"ioc".to_vec(), Some(1))
        );
        assert!(matches!(
            aa.mls.open_group(g, d, n),
            Err(GroupNotAvailable(_))
        ));
    }
}
