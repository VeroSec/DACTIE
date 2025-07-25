mod mls_crypto_provider;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::create_dir_all;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;
use openmls::{prelude::{*, tls_codec::*}};
use openmls::treesync::RatchetTree;
use openmls_basic_credential::SignatureKeyPair;
use thiserror::Error;
use libp2p::PeerId;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm,  Key
};
use aes_gcm::aead::generic_array::GenericArray;
use openmls::framing::Sender::Member;
use tokio::sync::{mpsc};
use tokio::time::{sleep,Duration};
use crate::shared_structs::Instruction;
use mls_crypto_provider::{MemoryStorageError, OpenMlsRustCrypto};
use MyOpenMLSError::*;
use thressig::structs::{EncShare, EncSharePairing, Signature};
use crate::key_store::StoredKeyMaterial;
use crate::shared_structs;

const NUM_CHANNELS: u64 = 10;
//NETWORK_DELAY in seconds, makes sure that all nodes have received the necessary proposals
const NETWORK_DELAY: u64 = 2;

pub struct MyOpenMls {
    ciphersuite: Ciphersuite,
    signer: SignatureKeyPair,
    credential_with_key: CredentialWithKey,
    provider: OpenMlsRustCrypto,
    //All Stores should be stored together with the MLS Provider in one SQLDB for Produciton
    key_package_store: Mutex<HashMap<PeerId,KeyPackage>>,
    group_store: Mutex<HashMap<GroupId,GroupStorage>>,
    group_ids: Mutex<Vec<GroupId>>,
    key_store: Mutex<StoredKeyMaterial>,
    subscribed_topics: Mutex<HashMap<String,Vec<Vec<u8>>>>,
    group_commit_tx: mpsc::UnboundedSender<GroupId>,
    instruction_tx: mpsc::UnboundedSender<Instruction>,
    load_files_path: PathBuf,
    common_group: Mutex<Option<GroupId>>,
    authority_id: PeerId,
    opener_pubkeys:  Mutex<Vec<thressig::G2>>,
}

impl MyOpenMls {
    pub fn new(key_store: StoredKeyMaterial, group_commit_tx: mpsc::UnboundedSender<GroupId>,instruction_tx: mpsc::UnboundedSender<Instruction>, load_files_path: PathBuf, authority_id: PeerId) -> Result<MyOpenMls, MyOpenMLSError>{
        let ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
        let mut group_path = load_files_path.clone();
        group_path.push("group_saves");

        let (common_group,group_ids,group_store, provider) =
            Self::load_from_file(group_path.clone(),instruction_tx.clone())
                .unwrap_or((Mutex::new(None),Mutex::new(Vec::new()),Mutex::new(HashMap::new()), OpenMlsRustCrypto::default()));


        let (credential_with_key, signer) = generate_openmls_credentials(&provider, key_store.get_skp(),key_store.get_peer_id())?;

        Ok(MyOpenMls{
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
        })
    }

    pub fn generate_join_key_material(&self, id: usize, n: thressig::G1, grpkey: Vec<u8>) -> Result<(thressig::G1,thressig::G1, (thressig::FieldElement,thressig::FieldElement),(thressig::FieldElement,thressig::FieldElement),Vec<EncShare>,Vec<thressig::G1>), MyOpenMLSError>{
        let opener_pubkeys= self.opener_pubkeys.lock().unwrap().clone();
        let res = self.key_store.lock().unwrap().generate_join_key_material(id, n, grpkey, &opener_pubkeys);
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)?;
        Ok(res)
    }

    pub fn generate_enc_shares(&self) -> Result<(Vec<EncShare>,Vec<thressig::G1>),MyOpenMLSError>{
        let opener_pubkeys = self.opener_pubkeys.lock().unwrap().clone();
        let mut key_store = self.key_store.lock().unwrap();
        let grpkey = key_store.get_grpkey().ok_or(GPKUndefinedError)?;
        let sk = key_store.get_memkey().ok_or(GSKUndefinedError)?.sk;
        let id = key_store.get_id().ok_or(IDUndefinedError)?;
        Ok(key_store.generate_enc_shares(grpkey,sk,id,&opener_pubkeys))
    }

    //Generate Keypair used for encryption of the shares
    pub fn generate_elgamal_key_pair(&self, ) -> Result<(), MyOpenMLSError>{
        self.key_store.lock().unwrap().generate_elgamal_key_pair().map_err(|_| GPKUndefinedError)?;
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)
    }

    pub fn get_elgamal_pubkey(&self, ) -> Result<thressig::G2, MyOpenMLSError>{
        Ok(self.key_store.lock().unwrap().get_elgamal_pubkey().ok_or(ElgamalUndefinedError)?.clone())
    }

    pub fn get_peerid(&self, ) -> PeerId{
        self.key_store.lock().unwrap().get_peer_id()
    }

    pub fn get_authorityid(&self, ) -> PeerId{
        self.authority_id
    }

    pub fn set_grpkey(&self, grpkey: Vec<u8>) -> Result<(), MyOpenMLSError> {
        self.key_store.lock().unwrap().set_grpkey(grpkey);
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)?;
        Ok(())
    }

    pub fn set_memkey(&self, memkey: Vec<u8>) -> Result<(), MyOpenMLSError> {
        self.key_store.lock().unwrap().set_memkey(memkey);
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)?;
        Ok(())
    }

    pub fn set_id(&self, id : usize) -> Result<(), MyOpenMLSError> {
        self.key_store.lock().unwrap().set_id(id);
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)?;
        Ok(())
    }


    pub fn add_partial_memkey(&self, partial_memkey: Vec<u8>) -> Result<(), MyOpenMLSError>{
        self.key_store.lock().unwrap().add_partial_memkey(partial_memkey);
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)?;
        Ok(())
    }

    pub fn add_opener_pubkeys(&self, opener_pubkeys: &[thressig::G2]) -> Result<(), MyOpenMLSError> {
        self.opener_pubkeys.lock().unwrap().extend_from_slice(opener_pubkeys);
        let mut path = self.load_files_path.clone();
        path.push("key_material.json");
        self.key_store.lock().unwrap().to_file(path).map_err(|_| KeyMaterialFileError)?;
        Ok(())
    }

    pub fn store_to_file(&self) -> Result<(),MyOpenMLSError>{
        let map =  self.group_store.lock().unwrap();
        let vec = self.subscribed_topics.lock().unwrap();
        let groups: Vec<_> = map.values().map(|x| (&x.group,&x.owned) ).collect();
        let mut topics = Vec::new();
        for topic in vec.values(){
            topics.extend(topic);
        }
        let x = serde_json::to_vec(&(self.common_group.lock().unwrap().clone(),groups, topics)).map_err(|e| SerializationError(e.to_string()))?;
        let mut path = self.load_files_path.clone();
        path.push("group_saves");
        if !path.exists() {
            create_dir_all(path.clone())?;
        }
        let mut ms_path = path.clone();
        ms_path.push("mstore");
        fs::write(ms_path,x)?;
        let mut p_path = path.clone();
        p_path.push("provider");
        self.provider.save_keystore(p_path).map_err(|reason|StoreError {reason})?;

        Ok(())
    }

    fn load_from_file(dir: PathBuf, instruction_tx: mpsc::UnboundedSender<Instruction>) -> Result<(Mutex<Option<GroupId>>,Mutex<Vec<GroupId>>, Mutex<HashMap<GroupId,GroupStorage>>, OpenMlsRustCrypto),MyOpenMLSError>{
        let mut path = dir.clone();
        path.push("mstore");
        let x = fs::read(path)?;
        let (common_group, groups, topics): (Option<GroupId>,Vec<(MlsGroup,bool)>, Vec<Vec<u8>>) = serde_json::from_slice(&x).map_err(|e| SerializationError(e.to_string()))?;
        let mut group_ids = Vec::new();
        let mut group_store = HashMap::new();
        let mut path = dir.clone();
        path.push("provider");



        let mut provider = OpenMlsRustCrypto::default();
        provider.load_keystore(path).map_err(|e| LoadError {reason:e})?;

        for (group,owned) in groups{
            let group_id = group.group_id().clone();
            if let Some(comm) = common_group.clone(){
                if comm != group_id{
                    group_ids.push(group_id.clone());
                }
            }


            let secret = group.export_secret(&provider,"Test","Test".as_bytes(), 32)?;
            let key =  Key::<Aes256Gcm>::from_slice(&secret);
            let aes_cipher = Aes256Gcm::new(&key);

            let gs = GroupStorage{group,aes_cipher,owned,added_pks: HashMap::new()};
            group_store.insert(group_id.clone(), gs);
        }

        for topic in topics{
            instruction_tx.send(Instruction::Subscribe(topic)).map_err(|e| LoadError {reason: e.to_string()})?;
        }
        Ok((Mutex::new(common_group),Mutex::new(group_ids),Mutex::new(group_store), provider))
    }

    /*
    Moves a Group to the common_group location
     */
    pub fn move_to_common(&self, group_number: usize){
        let mut vec = self.group_ids.lock().unwrap();
        let mut common = self.common_group.lock().unwrap();
        *common = Some(vec.remove(group_number));
    }

    pub fn move_to_common_id(&self, group_id: Vec<u8>) -> Result<(), MyOpenMLSError>{
        let group_id = GroupId::from_slice(&group_id);
        let position = self.group_ids.lock().unwrap().iter().position(|x| *x == group_id).ok_or(GroupNotAvailable("".to_string()))?;
        self.move_to_common(position);
        Ok(())
    }

    pub fn create_empty_mls_group(&self) -> Result<usize, MyOpenMLSError>{

        let group= MlsGroup::new(
            &self.provider,
            &self.signer,
            &MlsGroupCreateConfig::default(),
            self.credential_with_key.clone(),
        ).map_err(|e| CreateGroupError {reason: e.to_string()})?;


        let secret = group.export_secret(&self.provider,"Test","Test".as_bytes(), 32).map_err(|e| CreateGroupError {reason: e.to_string()})?;
        let key =  Key::<Aes256Gcm>::from_slice(&secret);
        let aes_cipher = Aes256Gcm::new(&key);

        let group_id = group.group_id().clone();

        {
            let mut map = self.group_store.lock().unwrap();
            map.insert(group_id.clone(), GroupStorage { group, aes_cipher, owned: true , added_pks: HashMap::new()});
        }


        let mut vec = self.group_ids.lock().unwrap();
        vec.push(group_id.clone());
        Ok(vec.len()-1)
    }

    pub fn create_mls_group(&self, peer_ids: &Vec<PeerId>) -> Result<usize, MyOpenMLSError>{
        //Get Keypackages
        let pairs = self.get_keypackages_for_peer_ids(peer_ids)?;

        let group_numb = self.create_empty_mls_group()?;

        let (peer_ids,commit,welcome,epoch) =
            self.add_group_members(self.get_group_id(group_numb)?, &pairs)?;

        let group_id = self.get_group_id(group_numb)?;

        self.instruction_tx.send(Instruction::Commit {group_id,peer_ids,commit,welcome,epoch})
            .map_err(|e|CreateGroupError {reason:e.to_string()})?;

        Ok(group_numb)

    }

    pub fn remove_mls_group(&self, group_id_vec: Vec<u8>) {
        let group_id= GroupId::from_slice(&group_id_vec);
        let mut map = self.group_store.lock().unwrap();
        map.remove(&group_id);
        let mut vec = self.group_ids.lock().unwrap();
        vec.retain(|x| x.to_vec() != group_id_vec);
    }

    pub fn add_group_from_welcome(&self, serialized_welcome: Vec<u8>, serialized_ratchet_tree: Vec<u8>) -> Result<Vec<u8>,MyOpenMLSError>{
        // de-serialize the message as an [`MlsMessageIn`] ...
        let mls_message_in = MlsMessageIn::tls_deserialize(&mut serialized_welcome.as_slice()).map_err(|x|  SerializationError(x.to_string()))?;
        let welcome = match mls_message_in.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            // We know it's a welcome message, so we ignore all other cases.
            _ => unreachable!("Unexpected message type."),
        };

        // ... and inspect the message.
        let ratchet_tree:RatchetTree = bincode::deserialize(&serialized_ratchet_tree).map_err(|x|  SerializationError(x.to_string()))?;

        let staged_join = StagedWelcome::new_from_welcome(
            &self.provider,
            &MlsGroupJoinConfig::default(),
            welcome.clone(),
            // The public tree is need and transferred out of band.
            // It is also possible to use the [`RatchetTreeExtension`]
            Some(ratchet_tree.into()),
        ).map_err(|e| JoinGroupError {reason: e.to_string()})?;

        let group = staged_join
            .into_group(&self.provider)
            .map_err(|e| JoinGroupError {reason:e.to_string()})?;

        let secret = group.export_secret(&self.provider,"Test","Test".as_bytes(), 32)?;
        println!("Secret: {secret:?}");
        let key =  Key::<Aes256Gcm>::from_slice(&secret);
        let aes_cipher = Aes256Gcm::new(&key);

        let group_id = group.group_id().clone();

        {
            let mut map = self.group_store.lock().unwrap();
            map.insert(group_id.clone(), GroupStorage { group, aes_cipher, owned: false , added_pks: HashMap::new()});
        }

        {
            let mut vec = self.group_ids.lock().unwrap();
            vec.push(group_id.clone());
        }

        Ok(group_id.to_vec())

    }

    pub fn get_group_id(&self, group_number: usize) -> Result<GroupId, MyOpenMLSError>{
        let vec = self.group_ids.lock().unwrap();
        Ok(vec.get(group_number).ok_or(GroupNotAvailable("".to_string()))?.clone())
    }

    pub fn get_common_group_id(&self) -> Result<GroupId, MyOpenMLSError>{
        self.common_group.lock().unwrap().clone().ok_or(CommonGroupUndefinedError)
    }

    pub fn add_group_members(&self, group_id: GroupId, pairs: &[(PeerId,KeyPackage)]) -> Result<(Vec<PeerId>,Vec<u8>, Option<Vec<u8>>, u64), MyOpenMLSError> {
        let mut map = self.group_store.lock().unwrap();
        let group_store =
            map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;

        let epoch = group_store.group.epoch().as_u64();

        let key_packages: Vec<KeyPackage> = pairs.iter().map(|(_, k)| k.clone()).collect();
        let peer_ids = pairs.iter().map(|(p, _)| p.clone()).collect();

        let (mls_message_out, welcome_out, _) = group_store.group.add_members(&self.provider, &self.signer, &key_packages).map_err(|e| CreateGroupError {reason: e.to_string()})?;
        group_store.group.merge_pending_commit(&self.provider).map_err(|e| CreateGroupError {reason: e.to_string()})?;
        let commit = mls_message_out.tls_serialize_detached().map_err(|e| SerializationError(e.to_string()))?;

        for (peer_id, key_package) in pairs {
            group_store.added_pks.insert(key_package.leaf_node().signature_key().clone(), peer_id.clone());
        }

        group_store.update_cipher(&self.provider)?;

        let welcome = Some(welcome_out.tls_serialize_detached().map_err(|e| SerializationError(e.to_string()))?);


        Ok((peer_ids ,commit, welcome, epoch))
    }

    pub fn propose_add_group_member(&self, group_number: usize, peer_id: PeerId) -> Result<(Vec<u8>, Vec<u8>, u64), MyOpenMLSError>{
        let key_package = self.get_keypackage(peer_id).ok_or(MissingKeyPackagesError {peer_ids:vec![peer_id]})?;

        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec.get(group_number).ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage { group, owned, added_pks, ..} =
            map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;

        let epoch = group.epoch().as_u64();

        let (mls_message_out,_) =group.propose_add_member(&self.provider, &self.signer, &key_package)?;
        let serialized_message = mls_message_out.tls_serialize_detached().map_err(|e|SerializationError(e.to_string()))?;

        added_pks.insert(key_package.leaf_node().signature_key().clone(),peer_id);
        self.notify_owned_proposal(*owned, group_id.clone())?;


        Ok((group_id.to_vec(),serialized_message, epoch))
    }

    pub fn remove_group_member(&self, group_number: usize, peer_id: PeerId) -> Result<(), MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec.get(group_number).ok_or(GroupNotAvailable("".to_string()))?;
        let group_store = map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;


        let index = group_store.group.members().position(|x| x.credential.serialized_content() == peer_id.to_bytes() ).ok_or(NotAGroupMemberError {peer_id})?;
        if index == 0 {
            return Err(RemoveGroupOwnerError)
        }
        let mut res = Vec::new();

        if let Some(pending_commit) = group_store.group.pending_commit() {
            for proposal in pending_commit.add_proposals() {
                if let Some(peer_id) = group_store.added_pks.remove(proposal.add_proposal().key_package().leaf_node().signature_key()) {
                    res.push(peer_id);
                }
            }
        }

        let (mls_message_out,welcome_option,_) =
            group_store.group.remove_members(&self.provider, &self.signer, &[LeafNodeIndex::new(index.try_into().map_err(|_| RemoveGroupMemberError)?)])
                .map_err(|_| RemoveGroupMemberError)?;
        let commit = mls_message_out.tls_serialize_detached().map_err(|e|SerializationError(e.to_string()))?;
        group_store.group.merge_pending_commit(&self.provider).map_err(|e| CreateGroupError {reason: e.to_string()})?;
        group_store.update_cipher(&self.provider)?;

        let epoch = group_store.group.epoch().as_u64();

        let welcome = welcome_option
            .map(|welcome_out| welcome_out.tls_serialize_detached().map_err(|e| SerializationError(e.to_string())))
            .transpose()?;

        self.instruction_tx.send(Instruction::Commit {group_id:group_id.clone(), peer_ids: res ,commit, welcome, epoch})
            .map_err(|e|CommitError {group_id: group_id.clone(), reason: e.to_string()})

    }

    pub fn propose_remove_group_member(&self, group_number: usize, peer_id: PeerId) -> Result<(Vec<u8>, Vec<u8>, u64), MyOpenMLSError> {
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec.get(group_number).ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage { group, owned, ..} = map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;


        let index = group.members().position(|x| x.credential.serialized_content() == peer_id.to_bytes() ).ok_or(NotAGroupMemberError {peer_id})?;
        if index == 0 {
            return Err(RemoveGroupOwnerError)
        }
        let (mls_message_out,_) =
            group.propose_remove_member(&self.provider,&self.signer,LeafNodeIndex::new(index.try_into().map_err(|_| RemoveGroupMemberError)?))
                .map_err(|_| RemoveGroupMemberError)?;
        let serialized_message = mls_message_out.tls_serialize_detached().map_err(|e|SerializationError(e.to_string()))?;
        self.notify_owned_proposal(*owned, group_id.clone())?;

        let epoch = group.epoch().as_u64();

        Ok((group_id.to_vec(),serialized_message,epoch))
    }

    pub fn update_key_material(&self, group_number: usize) -> Result<(Vec<u8>,Vec<u8>,u64), MyOpenMLSError>{
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec.get(group_number).ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage { group, owned, ..} = map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;

        let (mls_message_out,_) = group.propose_self_update(&self.provider,&self.signer, LeafNodeParameters::default()).map_err(|_| GroupUpdateError)?;
        let serialized_message = mls_message_out.tls_serialize_detached().map_err(|e|SerializationError(e.to_string()))?;
        self.notify_owned_proposal(*owned, group_id.clone())?;

        let epoch = group.epoch().as_u64();
        Ok((group_id.to_vec(),serialized_message, epoch))

    }

    pub fn delete_group_for_all(&self, group_number: usize)-> Result<Vec<u8>, MyOpenMLSError>{
        let vec = self.group_ids.lock().unwrap();
        let mut map = self.group_store.lock().unwrap();
        let group_id = vec.get(group_number).ok_or(GroupNotAvailable("".to_string()))?;
        let GroupStorage{group, owned,..} = map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;

        if !*owned{
            return Err(NotGroupOwnerError)
        }

        let other_peers: Vec<LeafNodeIndex> = group.members().skip(1).map(|x| x.index).collect();
        //Return early if group already empty
        if other_peers.is_empty(){
            return Ok(group_id.to_vec())
        }
        // Clear Proposal Store
        group.commit_to_pending_proposals(&self.provider,&self.signer).map_err(|e| DeleteAllError {reason: e.to_string()})?;
        group.clear_pending_commit(self.provider.storage())?;
        let (mls_message_out ,_,_) =group.remove_members(&self.provider, &self.signer, &other_peers).map_err(|_| RemoveGroupMemberError)?;

        let commit = mls_message_out.tls_serialize_detached().map_err(|e| SerializationError(e.to_string()))?;

        let epoch = group.epoch().as_u64();

        self.instruction_tx.send(Instruction::Commit {group_id:group_id.clone(), peer_ids: Vec::new() ,commit, welcome:None, epoch})
            .map_err(|e|CommitError {group_id: group_id.clone(), reason:e.to_string()})?;

        Ok(group_id.to_vec())
    }

    pub fn store_proposal(&self, group_id_vec: Vec<u8>, proposal_vec: Vec<u8>) -> Result<(), MyOpenMLSError>{
        let group_id= GroupId::from_slice(&group_id_vec);
        let mut map = self.group_store.lock().unwrap();
        let GroupStorage{group,owned, ..} = map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;

        let mls_message_in = MlsMessageIn::tls_deserialize_exact(&proposal_vec).map_err(|e|SerializationError(e.to_string()))?;
        let priv_message_in = match mls_message_in.extract(){
            MlsMessageBodyIn::PrivateMessage(priv_msg) => priv_msg,
            _ => return Err(InvalidPrivMessage("".to_string()))

        };
        let protocol_message = ProtocolMessage::from(priv_message_in);


        match group.process_message(&self.provider, protocol_message)?.into_content() {
            ProcessedMessageContent::ProposalMessage(staged_proposal) => {
                if let Proposal::Remove(prop) = (*staged_proposal.clone()).proposal(){
                    //Only add proposal, if not group Owner
                    if prop.removed().usize() == 0{
                        return Ok(())
                    }

                }
                group.store_pending_proposal(self.provider.storage(),*staged_proposal)?

            }
            _ => return Err(InvalidProposal("".to_string()))

        }
        if *owned {
            self.group_commit_tx.send(group_id.clone()).map_err(|e|CommitError { group_id, reason:e.to_string() })?;
        }

        Ok(())

    }

    pub async fn commit(&self, group_id: GroupId) -> Result<(), MyOpenMLSError> {
        let mut res = Vec::new();

        //makes sure that map is not locked during sleep
        let (mls_message_out, welcome_option, epoch) = {
            let mut map = self.group_store.lock().unwrap();
            let group_store = map.get_mut(&group_id).ok_or(GroupNotAvailable(hex::encode(group_id.to_vec())))?;
            let group = &mut group_store.group;
            let (mls_message_out, welcome_option, _) = group.commit_to_pending_proposals(&self.provider, &self.signer).map_err(|e| CommitError { group_id: group_id.clone() , reason: e.to_string()})?;

            if let Some(pending_commit) = group.pending_commit() {
                for proposal in pending_commit.add_proposals() {
                    if let Some(peer_id) = group_store.added_pks.remove(proposal.add_proposal().key_package().leaf_node().signature_key()) {
                        res.push(peer_id);
                    }
                }
            }

            if let Err(e) = group.merge_pending_commit(&self.provider) {
                return Err(CommitError { group_id: group_id.clone(), reason: e.to_string()});
            }
            let epoch = group.epoch().as_u64();
            group_store.update_cipher(&self.provider)?;
            (mls_message_out, welcome_option, epoch)
        };


        sleep(Duration::from_secs(NETWORK_DELAY)).await;

        let commit = mls_message_out.tls_serialize_detached().map_err(|e| SerializationError(e.to_string()))?;

        let welcome = welcome_option
            .map(|welcome_out| welcome_out.tls_serialize_detached().map_err(|e| SerializationError(e.to_string())))
            .transpose()?;



        self.instruction_tx.send(Instruction::Commit {group_id:group_id.clone(), peer_ids: res ,commit, welcome, epoch})
            .map_err(|e|CommitError {group_id, reason: e.to_string()})
    }

    pub fn get_ratchet_tree (&self,group_id_vec: &Vec<u8>) -> Result<Vec<u8>,MyOpenMLSError>{
        let group_id= GroupId::from_slice(&group_id_vec);
        let mut map = self.group_store.lock().unwrap();
        let group_option = map.get_mut(&group_id);
        match group_option {
            Some(group_store) => Ok(bincode::serialize(&group_store.group.export_ratchet_tree()).map_err(|e| SerializationError(e.to_string()))?),
            None => Err(GroupNotAvailable("Could not find group for ratchet tree export".to_string()))
        }

    }

    /*
    Merges own Group with Commit and Returns a Vec of Peers to send Welcome Message to
     */
    pub fn handle_commit(&self, group_id_vec: Vec<u8>, commit_vec: Vec<u8>) -> Result<(Vec<PeerId>, Vec<u8>, bool), MyOpenMLSError>{

        let mut res = Vec::new();
        let mls_message_in =MlsMessageIn::tls_deserialize_exact(&commit_vec).map_err(|e|SerializationError(e.to_string()))?;

        let priv_message_in = match mls_message_in.extract(){
            MlsMessageBodyIn::PrivateMessage(priv_msg) => priv_msg,
            _ => {
                return Err(InvalidPrivMessage("".to_string()));
            }
        };
        let protocol_message = ProtocolMessage::from(priv_message_in);

        let group_id= GroupId::from_slice(&group_id_vec);
        let (ratchet, self_removed) = {
            let mut map = self.group_store.lock().unwrap();
            let group_store = map.get_mut(&group_id).ok_or(GroupNotAvailable("".to_string()))?;

            let processed_message = group_store.group.process_message(&self.provider, protocol_message).map_err(|e| MyProcessMessageError(e))?;
            // Only handle the commit if it is from the group owner. According to the RFC this should be secure: https://datatracker.ietf.org/doc/html/rfc9420#name-authentication
            // The leaf not index is not fixed in the RFC. In this OpenMls version it does not change
            if !matches!(processed_message.sender(), Member(node_index) if node_index.usize() == 0) {
                return Err(NonGroupOwnerCommitError { sender: processed_message.sender().clone() });
            }


            match  processed_message.into_content(){
                ProcessedMessageContent::StagedCommitMessage(staged_commit) => {

                    //If I added a peer make sure to send him the Welcome Message
                    for proposal in staged_commit.add_proposals(){
                        if let Some(peer_id) = group_store.added_pks.remove(proposal.add_proposal().key_package().leaf_node().signature_key()){
                            res.push(peer_id)
                        }
                    }
                    let self_removed = staged_commit.self_removed();
                    group_store.group.merge_staged_commit(&self.provider, *staged_commit).map_err(|e| CommitError {group_id: group_id.clone(),reason: e.to_string()})?;
                    let res_tree =
                        bincode::serialize(&group_store.group.export_ratchet_tree()).map_err(|e| SerializationError(e.to_string()))?;
                    if !self_removed{
                        group_store.update_cipher(&self.provider)?;
                    }
                    (res_tree,self_removed)
                }
                _ => {
                    return Err(InvalidProposal("".to_string()));
                }

            }

        };

        Ok((res, ratchet, self_removed))
    }

    //Stores a KeyPackage for further use
    pub fn insert_foreign_kp(&self, key:PeerId, value:KeyPackage) {
        let mut map = self.key_package_store.lock().unwrap();
        map.insert(key, value);
    }

    //Returns a list of all Groups
    pub fn get_groups(&self) -> Vec<GroupId>{
        let vec = self.group_ids.lock().unwrap();
        vec.clone()
    }

    //Returns the epoch of a group
    pub fn get_group_epoch(&self, group_id: &GroupId) -> Option<u64>{
        let map = self.group_store.lock().unwrap();
        map.get(group_id).map(|x| x.group.epoch().as_u64())
    }

    pub fn sign<T>(&self, body: T) -> Result<T::Output, MyOpenMLSError>
    where
        T: shared_structs::Signable + serde::Serialize,
    {
        let (grpkey,memkey)= {
            let key_store= self.key_store.lock().unwrap();
            (key_store.get_grpkey().ok_or(GPKUndefinedError)?,key_store.get_memkey().ok_or(GSKUndefinedError)?)
        };
        let signature = thressig::sign(&memkey, &grpkey,&body)?;
        Ok(T::new(body, signature))
    }

    pub fn verify<T>(&self, message: T) -> Result<(T::Body,Signature), MyOpenMLSError>
    where
        T: shared_structs::Verifiable, <T as shared_structs::Verifiable>::Body: serde::Serialize
    {
        let grpkey= self.key_store.lock().unwrap().get_grpkey().ok_or(GPKUndefinedError)?;
        let (body,signature) = message.into_parts();
        let signature = signature.ok_or(SignatureMissing)?;
        thressig::verify(&signature,&grpkey, &body).map_err(|_| SignatureInvalid{signature:signature.clone()})?;
        Ok((body, signature))
    }
    
    pub fn verify_enc_share(&self,enc_share: &EncShare,commits: &[thressig::G1], elgamal_pk: Option<thressig::G2>) -> bool {
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

            (group_key,elgamal_pk)
        };
    thressig::verify_enc_share_correctness(enc_share,&group_key,&elgamal_pk,commits)
    }

    pub fn open<K>(&self, signature: &Signature,gml: HashSet<K>, all_sj: HashMap<usize, Vec<(K, thressig::GT)>>) -> Vec<K>
        where
            K: Eq+Clone,
    {

        let group_key = match {self.key_store.lock().unwrap().get_grpkey()}  {
            Some(v) => v,
            None => return Vec::new(),
        };

        thressig::open(signature, &group_key, &gml, &all_sj)

    }


    /*Encrypts a message for specific group
    Returns: GroupId + Ciphertext + Nonce*/
    pub fn encrypt_with_group_key(&self, group_id: GroupId, plaintext: Vec<u8>) -> Result<(Vec<u8>,Vec<u8>, Vec<u8>), MyOpenMLSError> {
        let to_encrypt = bincode::serialize(&plaintext).map_err(|e|SerializationError(e.to_string()))?;

        let map = self.group_store.lock().unwrap();
        let GroupStorage{aes_cipher,..} = map.get(&group_id).ok_or(GroupNotAvailable("".to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = aes_cipher.encrypt(&nonce, to_encrypt.as_ref()).unwrap();
        Ok((group_id.to_vec(),ciphertext,nonce.to_vec()))
    }

    //Decrypts a message for specific group
    //Returns Group Number Option, because Common Group does not have a number
    pub fn decrypt_with_group_key(&self, group_id_vec: Vec<u8>, ciphertext: Vec<u8>, ser_nonce: Vec<u8>) -> Result<(Option<usize>, Vec<u8>), MyOpenMLSError> {
        let nonce = GenericArray::from_slice(&ser_nonce[..]);
        let group_id= GroupId::from_slice(&group_id_vec);
        let group_number = {
            let vec = self.group_ids.lock().unwrap();
            vec.iter().position(|r| *r == group_id)
        };



        let map = self.group_store.lock().unwrap();
        let GroupStorage{aes_cipher,..} = map.get(&group_id).ok_or(GroupNotAvailable(hex::encode(group_id.to_vec())))?;
        let plaintext: Vec<u8> = bincode::deserialize(&aes_cipher.decrypt(&nonce, ciphertext.as_ref()).expect("Error")).map_err(|e| SerializationError(e.to_string()))?;
        Ok((group_number,plaintext))
    }


    //Decrypts a message which arrives for the broadcast group
    pub fn decrypt_broadcast(&self, ciphertext: Vec<u8>, ser_nonce: Vec<u8>) -> Result<Vec<u8>, MyOpenMLSError> {

        let (_, plain) = self.decrypt_with_group_key(self.get_common_group_id()?.to_vec(), ciphertext,ser_nonce)?;
        Ok(plain)
    }

    // For a list of Peerids returns all available and missing keypackets, if missing is empty remove all keypackages from storage
    pub fn get_keypackages_for_peer_ids(&self, peer_ids: &Vec<PeerId>) -> Result<Vec<(PeerId,KeyPackage)>,MyOpenMLSError>{
        let mut map = self.key_package_store.lock().unwrap();
        let mut res = Vec::new();
        let mut missing = Vec::new();
        for peer_id in peer_ids{
            let keypackage = map.get(peer_id);
            if let Some(key) = keypackage {
                res.push((peer_id.clone(),key.clone()));
            }
            else { missing.push(peer_id.clone()) }
        }
        if !missing.is_empty() {
            return Err(MissingKeyPackagesError {peer_ids: missing})
        }
        for peer_id in peer_ids {
            map.remove(peer_id);
        }
        Ok(res)

    }

    // For a Peerids returns a keypackage
    pub fn get_keypackage(&self, peer_id: PeerId) -> Option<KeyPackage>{
        let mut map = self.key_package_store.lock().unwrap();
        map.remove(&peer_id)
    }

    // A helper to create key package bundles.
    pub fn generate_key_package(&self) -> Result<KeyPackageBundle,MyOpenMLSError> {
        // Create the key package
        KeyPackage::builder()
            .build(
                self.ciphersuite,
                &self.provider,
                &self.signer,
                self.credential_with_key.clone(),
            ).map_err(|_| NewKeyPackageError)
    }


    pub fn subscibed_to_topic(&self, topic: Vec<u8>) -> bool {
        let map = self.subscribed_topics.lock().unwrap();
        let channel = self.get_channel(topic.clone());
        map.get(&channel).map_or(false, |x| x.contains(&topic))
    }

    pub fn get_channel(&self, topic: Vec<u8>) -> String {
        let mut hasher = DefaultHasher::new();
        topic.hash(&mut hasher);
        format!("channel_{}",hasher.finish() % NUM_CHANNELS)
    }

    //Returns Channel, if not already subscribed
    pub fn subscribe(&self, topic: Vec<u8>) -> Result<Option<String>,MyOpenMLSError> {
        let mut map = self.subscribed_topics.lock().unwrap();
        let channel = self.get_channel(topic.clone());
        match map.get_mut(&channel) {
            Some(val) => {
                if val.contains(&topic) {
                    Err(AlreadySubscribedError {topic:topic.clone()})
                } else {
                    val.push(topic);
                    Ok(None)
                }
            },
            None => {
                map.insert(channel.clone(), vec![topic]);
                Ok(Some(channel))
            }
        }


    }

    //Returns Channel, if it has no subscribed topics and gossipsub can unsubscribe from it.
    pub fn unsubscribe(&self, topic: Vec<u8>) -> Result<Option<String>,MyOpenMLSError> {
        let mut map = self.subscribed_topics.lock().unwrap();
        let channel = self.get_channel(topic.clone());
        let channel_list =  map.get_mut(&channel).ok_or(NoSubscriptionError {topic:topic.clone()})?;
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
        signature: &Vec<u8>,
    ) -> Result<Vec<(K, EncSharePairing)>, MyOpenMLSError>
    where
        K: Clone,
    {
        let keystore = self.key_store.lock().unwrap();
        let secret_key = match keystore.get_elgamal_secretkey() {
            Some(v) => v,
            None => return Err(ElgamalUndefinedError),
        };

        let signature: Signature = bincode::deserialize(signature)
            .map_err(|e| SerializationError(e.to_string()))?;

        let enc_shares: Result<Vec<(K, EncSharePairing)>, bincode::Error> = enc_shares
            .iter()
            .map(|(id, enc_share)| {
                let enc_share: EncShare = bincode::deserialize(enc_share)?;
                let pairing = thressig::create_open_pairing(&signature, enc_share, secret_key);
                Ok((id.clone(), pairing))
            })
            .collect();

        Ok(enc_shares.map_err(|e| SerializationError(e.to_string()))?)
    }


    //Notifies the group owner, that there are new proposals
    fn notify_owned_proposal(&self, owned:bool, group_id: GroupId) -> Result<(),MyOpenMLSError>{
        if owned {
            self.group_commit_tx.send(group_id.clone()).map_err(|e| CommitError { group_id: group_id.clone() ,reason:e.to_string()})?
        }

        Ok(())
    }


}

fn generate_openmls_credentials(provider: &impl OpenMlsProvider, skp: SignatureKeyPair,peer_id: PeerId) -> Result<(CredentialWithKey, SignatureKeyPair), MyOpenMLSError >{
    let credential = BasicCredential::new(peer_id.to_bytes());
    // Store the signature key into the key store so OpenMLS has access
    // to it.
    skp
        .store(provider.storage())
        .map_err(|_| GenerateCredentialsError {reason:"Store to provider did not work".to_string()})?;

    Ok((
        CredentialWithKey {
            credential: credential.into(),
            signature_key: skp.public().into(),
        },
        skp,
    ))
}



struct GroupStorage {
    group:MlsGroup,
    aes_cipher: Aes256Gcm,
    owned: bool,
    added_pks: HashMap<SignaturePublicKey,PeerId>
}

impl GroupStorage {
    fn update_cipher(&mut self, provider: &OpenMlsRustCrypto) -> Result<(), MyOpenMLSError>{
        let secret = self.group.export_secret(provider,"Test","Test".as_bytes(), 32)?;
        println!("Secret: {secret:?}");
        let key =  Key::<Aes256Gcm>::from_slice(&secret);
        let aes_cipher = Aes256Gcm::new(&key);
        self.aes_cipher = aes_cipher;
        Ok(())

    }

}

#[derive(Debug, Error)]
pub enum MyOpenMLSError {
    #[error("Could not find group in storage")]
    GroupNotAvailable(String),
    #[error("The signature is invalid")]
    SignatureInvalid{signature:thressig::structs::Signature},
    #[error("No signature available")]
    SignatureMissing,
    #[error("Could not add members to group {group_id:?}, because {reason:?}")]
    GroupAddMember { group_id: GroupId , reason: String},
    #[error("Deserialization Error, in Encryption")]
    SerializationError(String),
    #[error("Not subscribed to topic: `{topic:?}`")]
    NoSubscriptionError{topic: Vec<u8>},
    #[error("Already subscribed to topic: `{topic:?}`")]
    AlreadySubscribedError {topic: Vec<u8>},
    #[error("Missing KeyPackages for the following Peers: `{peer_ids:?}`")]
    MissingKeyPackagesError {peer_ids: Vec<PeerId>},
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
    CommitError {group_id: GroupId, reason: String},
    #[error("Got a Commit from a not group owner {sender:?}")]
    NonGroupOwnerCommitError {sender: Sender},
    #[error("Peer {peer_id:?} is not a group member")]
    NotAGroupMemberError{peer_id: PeerId},
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
    CreateGroupError{reason: String},
    #[error("Could not store Group and topic information to file: `{reason:?}`")]
    StoreError{reason: String},
    #[error("Could not join Group from welcome: `{reason:?}`")]
    JoinGroupError{reason: String},
    #[error("Could not delete group for all: `{reason:?}`")]
    DeleteAllError{reason: String},
    #[error("Could not load Group Configuration from File: `{reason:?}`")]
    LoadError{reason: String},
    #[error("Could not generate Credentials: `{reason:?}`")]
    GenerateCredentialsError{reason: String},
    #[error("GroupMemberKey not set, can not connect do Dactie Network")]
    GSKUndefinedError,
    #[error("GroupPublicKey not set, can not connect do Dactie Network")]
    GPKUndefinedError,
    #[error("ID not set, can not go forward")]
    IDUndefinedError,
    #[error("Elgamalkeys not set")]
    ElgamalUndefinedError,
    #[error("Group Signature Operation failed: {0}")]
    PS16Error(#[from] thressig::ThressigError),
    #[error("CommonGroup Not Set")]
    CommonGroupUndefinedError,
    #[error("The path to the keystore is not valid or the file is corrupted")]
    KeyMaterialFileError,


}

