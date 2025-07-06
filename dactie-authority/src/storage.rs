use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Mutex;
use libp2p::identity::ed25519::PublicKey;
use libp2p::PeerId;
use openmls::prelude::KeyPackage;
use thressig::{FieldElement, G1, G2};
use thressig::structs::{EncShare, Grpkey, Mgrkey};
use thiserror::Error;
use dactie_utils::key_store::StoredKeyMaterial;
use crate::storage::MyStorageError::{CreateMemkeyError, FileAccessError, GetNError, IdPeerMapError, KeyPackageSenderError, SerializationError};
use serde::{Deserialize, Serialize};

/**
    The storage is used to save information about the added group members and the key material that is required to add new members.
 **/
#[derive(Serialize,Deserialize)]
pub(crate) struct Storage {
    n_store: Mutex<HashMap<PeerId, (G1,usize)>>,
    grpkey: Grpkey,
    mgrkey: Mgrkey,
    gml: Mutex<HashSet<usize>>,
    path_buf: PathBuf,
    archive_peers: Mutex<Vec<PeerId>>,
    allowed_peers: Mutex<Vec<PeerId>>,
    archive_init_mode: Mutex<bool>,
    opener_pubkeys: Mutex<Vec<G2>>,
}


impl Storage {
    pub(crate) fn new(path_buf: PathBuf) -> (Storage, StoredKeyMaterial){
        let (grpkey, mgrkey) = thressig::setup();

        let gml = HashSet::new();


        let key_material = StoredKeyMaterial::new(Some(grpkey.clone()),None, None);

        /* TODO Remove if not needed
        let peer_id = key_material.get_peer_id();
        let allowed_peers = vec![peer_id];
        let mut id_peer_map = HashMap::new();
        id_peer_map.insert(id, vec![peer_id]);
        */

        (Storage{n_store: Mutex::new(HashMap::new()), grpkey, mgrkey,gml:Mutex::new(gml), path_buf, allowed_peers: Mutex::new(Vec::new()),archive_peers: Mutex::new(Vec::new()), archive_init_mode: Mutex::new(true), opener_pubkeys: Mutex::new(Vec::new())}, key_material, )
    }

    pub(crate) fn id_exists(&self,id: &usize) -> bool{
        self.gml.lock().unwrap().contains(id)
    }

    pub(crate) fn get_archive_peer_ids(&self, ) -> Vec<PeerId>{
        self.archive_peers.lock().unwrap().clone()
    }

    pub(crate) fn add_archive(&self, peer_id: PeerId)  {
        let mut vec = self.archive_peers.lock().unwrap();
        if !vec.contains(&peer_id) {
            vec.push(peer_id);
        }
    }

    pub(crate) fn end_init_archive(&self,) {
        let mut mode = self.archive_init_mode.lock().unwrap();
        *mode = false;
    }

    pub(crate) fn get_opener_pubkeys(&self,) -> Vec<G2> {
        self.opener_pubkeys.lock().unwrap().clone()
    }


    // Opens a signature
    /* TODO Remove when archive implemented correctly
    pub(crate) fn open(&self, signature_vec: Vec<u8>) -> Result<usize, MyStorageError>{
        let signature = Signature::from_bytes(&signature_vec);
        let gml = self.gml.lock().unwrap();
        ps16::open(&signature, &self.grpkey, &gml).map_err(|_|SignatureOpenError).map(|(x,_)| x)
    } */

    // Adds new Peer to network
    pub(crate) fn allow_peer(&self, key_package: &KeyPackage, sender_peer_id: PeerId) -> Result<(),MyStorageError>{
        let pub_mls_key = key_package.leaf_node().signature_key().as_slice();
        let peer_id = libp2p::identity::PublicKey::from(PublicKey::try_from_bytes(pub_mls_key).map_err(|_|SerializationError {reason:"PeerID".to_string()})?).to_peer_id();

        if peer_id == sender_peer_id{
            {
                let mut vec = self.allowed_peers.lock().unwrap();
                if !vec.contains(&peer_id){
                    vec.push(peer_id)
                }
            }

        } else {
            return Err(KeyPackageSenderError {sender_peer_id, peer_id})
        }
        self.to_file()?;
        Ok(())
    }

    pub fn add_opener_pubkey(&self, opener_pubkey: G2) {
        self.opener_pubkeys.lock().unwrap().push(opener_pubkey);
    }



    pub fn remove_n(&self,peer_id: &PeerId) -> Option<(G1,usize)> {
        self.n_store.lock().unwrap().remove(peer_id)
    }

    //Generates n and stores it and returns it with a grpkey
    pub(crate) fn return_new_n(&self, peer_id: PeerId, id:usize)-> (Option<G1>, Vec<u8>){
        let n = &thressig::join_mgr1();
        self.n_store.lock().unwrap().insert(peer_id, (n.clone(),id));
        (Some(n.clone()),self.grpkey.to_bytes())
    }

    pub(crate) fn get_groupkey_as_bytes(&self) -> Vec<u8> {
        self.grpkey.to_bytes()
    }

    pub(crate) fn get_archive_init_mode(&self) -> bool {
        let mode = self.archive_init_mode.lock().unwrap();
        *mode
    }


    //Creates a Memkey from the member material
    //TODO send commits and enc to archives
    pub(crate) fn return_memkey(&self, peer_id: &PeerId, g_sk: &G1, h_sk: &G1, commits: &[G1], enc_shares: &[EncShare], pi1: (FieldElement,FieldElement), pi2: (FieldElement,FieldElement)) -> Result<(usize,Vec<u8>), MyStorageError>{
        let (n,id) = self.remove_n(peer_id).ok_or(GetNError {peer_id: peer_id.clone()})?;
        let opener_pubkeys = self.opener_pubkeys.lock().unwrap().clone();
        let part_memkey = thressig::join_mgr2(id,&mut self.gml.lock().unwrap(), &self.mgrkey, &self.grpkey, &n, g_sk, h_sk, &opener_pubkeys,commits,pi1, pi2, enc_shares)
            .map_err(|reason| CreateMemkeyError { reason })?;
        self.to_file()?;
        Ok((id, part_memkey.to_bytes()))
    }

    /**
    Functions to save storage to a file.
    **/

    pub(crate) fn to_file(&self) -> Result<(),MyStorageError>{
        let file = File::create(self.path_buf.clone()).map_err(|e|FileAccessError {reason:e.to_string()})?;
        serde_json::to_writer_pretty(file, self).map_err(|e|SerializationError {reason:e.to_string()})?;
        Ok(())
    }

    pub(crate) fn from_file(path :PathBuf) -> Result<Storage,MyStorageError>{
        let file = File::open(path).map_err(|e|FileAccessError {reason:e.to_string()})?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(|e|SerializationError {reason:e.to_string()})
    }
}
#[derive(Debug, Error)]
pub enum MyStorageError {
    #[error("Failed create Memkey: `{reason:?}`")]
    CreateMemkeyError{ reason: String },

    #[error("N for peer_id ´{peer_id:?}´ is not stored")]
    GetNError{ peer_id: PeerId },

    #[error("Signature could not be opened")]
    SignatureOpenError,

    #[error("Error occured while trying to interact with ID_Peer_Map")]
    IdPeerMapError,

    #[error("Got a keypackage with a different identity from a peer. Sender PeerID:{sender_peer_id}, KeyPackage PeerID: {peer_id}")]
    KeyPackageSenderError{sender_peer_id: PeerId, peer_id: PeerId},

    #[error("Could not serialize/deserialize {reason:?}")]
    SerializationError{reason: String},

    #[error("Could not access file: {reason:?}")]
    FileAccessError{reason: String},

}


