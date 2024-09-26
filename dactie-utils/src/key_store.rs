use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use ps16::{join_mem1};
use libp2p::identity::Keypair;
use libp2p::PeerId;
use openmls::prelude::Ciphersuite;
use openmls_basic_credential::SignatureKeyPair;
use ps16::structs::{Grpkey, Memkey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]

/**
File is used to store the group and signature key material of Sharing and Archive Peers.
**/

pub struct StoredKeyMaterial {
    grpkey: Option<Grpkey>,
    memkey: Option<Memkey>,
    signature_key_pair: SignatureKeyPair
}

impl StoredKeyMaterial {

    pub fn new(grpkey: Option<Grpkey>, memkey: Option<Memkey>) -> StoredKeyMaterial{
        let signature_key_pair = SignatureKeyPair::new(Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519.signature_algorithm())
            .expect("Could not generate Keypair");
        StoredKeyMaterial{grpkey,memkey, signature_key_pair}

    }

    pub fn get_grpkey(&self) -> Option<Grpkey> {
        self.grpkey.clone()
    }


    pub fn get_memkey(&self) -> Option<Memkey> {
        self.memkey.clone()
    }

    pub fn get_skp(&self) -> SignatureKeyPair {
        self.signature_key_pair.clone()
    }


    pub fn get_keypair(&self) -> Keypair {
        Keypair::ed25519_from_bytes(self.signature_key_pair.private()[..32].to_vec()).unwrap()
    }
    pub fn get_peer_id(&self) -> PeerId {
        Keypair::ed25519_from_bytes(self.signature_key_pair.private()[..32].to_vec()).unwrap().public().to_peer_id()
    }

    pub fn generate_join_key_material(&mut self, n: ps16::G1, grpkey: Vec<u8>) -> (ps16::G1,ps16::G2,(ps16::FieldElement,ps16::FieldElement)){
        let grpkey = Grpkey::from_bytes(&grpkey);
        let (tau, ttau, (p1,p2),sk) = join_mem1(&n, &grpkey);
        self.memkey = Some(Memkey{sk, sigma1: None, sigma2:None, e:None});
        self.grpkey = Some(grpkey);
        (tau, ttau, (p1,p2))
    }

    pub fn add_partial_memkey(&mut self, memkey: Vec<u8>) {
        let mut partial_memkey = Memkey::from_bytes(&memkey);
        partial_memkey.sk = self.memkey.clone().unwrap().sk;
        self.memkey = Some(partial_memkey);
    }

    pub fn from_file(path: PathBuf) -> Result<StoredKeyMaterial,Box<dyn std::error::Error>> {
        let key_material_file = OpenOptions::new()
            .read(true)
            .open(path)?;

        Ok(serde_json::from_reader(key_material_file)?)
    }



    //Sets the grpkey
    pub fn set_grpkey(&mut self, grpkey:Vec<u8>){
        let grpkey = Grpkey::from_bytes(&grpkey);
        self.grpkey = Some(grpkey);
    }

    //Sets the memkey
    pub fn set_memkey(&mut self, memkey:Vec<u8>){
        let memkey = Memkey::from_bytes(&memkey);
        self.memkey = Some(memkey);
    }

    pub fn to_file(&self, path :PathBuf) -> Result<(), String>{
        let file = File::create(path).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(file, self).map_err(|e| e.to_string())?;
        Ok(())
    }
}


