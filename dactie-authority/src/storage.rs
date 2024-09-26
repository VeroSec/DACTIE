use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Mutex;
use libp2p::identity::ed25519::PublicKey;
use libp2p::PeerId;
use openmls::prelude::KeyPackage;
use ps16::{FieldElement, G1, G2};
use ps16::structs::{Grpkey, Mgrkey};
use thiserror::Error;
use dactie_utils::key_store::StoredKeyMaterial;
use crate::storage::MyStorageError::{CreateMemkeyError, FileAccessError, GetNError, IdPeerMapError, KeyPackageSenderError, SerializationError, SignatureOpenError};
use serde::{Deserialize, Serialize};

/**
    The storage is used to save information about the added group members and the key material that is required to add new members.
 **/
#[derive(Serialize,Deserialize)]
pub(crate) struct Storage {
    n_store: Mutex<HashMap<PeerId, G1>>,
    grpkey: Grpkey,
    mgrkey: Mgrkey,
    gml: Mutex<Vec<(usize,G1,G2)>>,
    path_buf: PathBuf,
    id_peer_map: Mutex<HashMap<usize, Vec<PeerId>>>,
    allowed_peers: Mutex<Vec<PeerId>>
}


impl Storage {
    pub(crate) fn new(path_buf: PathBuf) -> (Storage, StoredKeyMaterial){
        let (grpkey, mgrkey) = ps16::setup();

        let mut gml = vec![];
        let n = ps16::join_mgr1();
        let (tau,ttau,pi,sk) = ps16::join_mem1(&n, &grpkey);
        let (id,partial_memkey) = ps16::join_mgr2(&mut gml, &mgrkey, &grpkey, &n, &tau, &ttau, pi).unwrap();
        let memkey = ps16::join_mem2(&sk, &partial_memkey);


        let key_material = StoredKeyMaterial::new(Some(grpkey.clone()),Some(memkey.clone()));


        let peer_id = key_material.get_peer_id();
        let allowed_peers = vec![peer_id];
        let mut id_peer_map = HashMap::new();
        id_peer_map.insert(id, vec![peer_id]);


        (Storage{n_store: Mutex::new(HashMap::new()), grpkey, mgrkey,gml:Mutex::new(gml), path_buf, allowed_peers: Mutex::new(allowed_peers),id_peer_map: Mutex::new(id_peer_map)}, key_material, )
    }

    pub(crate) fn get_peer_ids(&self,id: &usize) -> Option<Vec<PeerId>>{
        self.id_peer_map.lock().unwrap().get(id).cloned()
    }

    pub(crate) fn add_peer_id(&self,id: &usize, peer_id: PeerId, should_exist: bool) -> Result<(), MyStorageError> {
        let mut map = self.id_peer_map.lock().unwrap();
        if !should_exist && map.get(id).is_none() {
            map.insert(id.clone(), Vec::new());
        }
        map.get_mut(id).ok_or(IdPeerMapError)?.push(peer_id);
        Ok(())
    }


    // Opens a signature
    pub(crate) fn open(&self, signature_vec: Vec<u8>) -> Result<usize, MyStorageError>{
        let signature = ps16::structs::Signature::from_bytes(&signature_vec);
        let gml = self.gml.lock().unwrap();
        ps16::open(&signature, &self.grpkey, &gml).map_err(|_|SignatureOpenError).map(|(x,_)| x)
    }

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
        Ok(())
    }



    pub fn remove_n(&self,peer_id: &PeerId) -> Option<G1>{
        self.n_store.lock().unwrap().remove(peer_id)
    }

    //Generates n and stores it and returns it with a grpkey
    pub(crate) fn return_new_n(&self, peer_id: PeerId)-> (Option<G1>, Vec<u8>){
        let n = &ps16::join_mgr1();
        self.n_store.lock().unwrap().insert(peer_id, n.clone());
        (Some(n.clone()),self.grpkey.to_bytes())
    }

    //Creates a Memkey from the member material
    pub(crate) fn return_memkey(&self, peer_id: &PeerId, tau: G1, ttau: G2, pi1: FieldElement, pi2: FieldElement) -> Result<(usize,Vec<u8>), MyStorageError>{
        let n = self.remove_n(peer_id).ok_or(GetNError {peer_id: peer_id.clone()})?;
        let (id, part_memkey) = ps16::join_mgr2(&mut self.gml.lock().unwrap(), &self.mgrkey, &self.grpkey, &n, &tau, &ttau, (pi1, pi2))
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


