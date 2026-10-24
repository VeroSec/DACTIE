use libp2p::identity::Keypair;
use libp2p::PeerId;
use openmls::prelude::Ciphersuite;
use openmls_basic_credential::SignatureKeyPair;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use thressig::structs::{EncShare, Grpkey, Memkey};
use thressig::{create_enc_shares, join_mem1, FieldElement, G2};

pub type JoinKeyMaterial = (
    thressig::G1,
    thressig::G1,
    (FieldElement, FieldElement),
    (FieldElement, FieldElement),
    (FieldElement, FieldElement),
    Vec<EncShare>,
    Vec<thressig::G1>,
);

#[derive(Debug, Serialize, Deserialize, Clone)]
/**
File is used to store the group and signature key material of Sharing and Archive Peers.
**/
pub struct StoredKeyMaterial {
    grpkey: Option<Grpkey>,
    memkey: Option<Memkey>,
    elgamal_keypair: Option<(FieldElement, G2)>,
    signature_key_pair: SignatureKeyPair,
    id: Option<usize>,
}

impl StoredKeyMaterial {
    pub fn new(
        grpkey: Option<Grpkey>,
        memkey: Option<Memkey>,
        elgamal_keypair: Option<(FieldElement, G2)>,
        id: Option<usize>,
    ) -> StoredKeyMaterial {
        let signature_key_pair = SignatureKeyPair::new(
            Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519.signature_algorithm(),
        )
        .expect("Could not generate Keypair");
        StoredKeyMaterial {
            grpkey,
            memkey,
            elgamal_keypair,
            signature_key_pair,
            id,
        }
    }

    pub fn get_grpkey(&self) -> Option<Grpkey> {
        self.grpkey.clone()
    }

    pub fn get_memkey(&self) -> Option<Memkey> {
        self.memkey.clone()
    }

    pub fn get_id(&self) -> Option<usize> {
        self.id
    }

    pub fn get_skp(&self) -> SignatureKeyPair {
        self.signature_key_pair.clone()
    }

    pub fn get_elgamal_pubkey(&self) -> Option<&G2> {
        self.elgamal_keypair.as_ref().map(|t| &t.1)
    }

    pub fn get_elgamal_secretkey(&self) -> Option<&FieldElement> {
        self.elgamal_keypair.as_ref().map(|t| &t.0)
    }

    pub fn get_keypair(&self) -> Keypair {
        Keypair::ed25519_from_bytes(self.signature_key_pair.private()[..32].to_vec()).unwrap()
    }
    pub fn get_peer_id(&self) -> PeerId {
        Keypair::ed25519_from_bytes(self.signature_key_pair.private()[..32].to_vec())
            .unwrap()
            .public()
            .to_peer_id()
    }

    pub fn generate_join_key_material(
        &mut self,
        id: usize,
        n: thressig::G1,
        grpkey: Vec<u8>,
        opener_pubkeys: &[G2],
        threshold: usize,
    ) -> Result<JoinKeyMaterial, String> {
        let grpkey = Grpkey::from_bytes(&grpkey).map_err(|e| format!("group key: {e}"))?;
        let (g_sk, h_sk, pi1, pi2, pi3, sk, enc_shares, commits) =
            join_mem1(id, n, &grpkey, threshold, opener_pubkeys);
        self.memkey = Some(Memkey {
            sk,
            sigma1: None,
            sigma2: None,
            e1: None,
        });
        self.grpkey = Some(grpkey);
        Ok((g_sk, h_sk, pi1, pi2, pi3, enc_shares, commits)) //Return everything except sk
    }

    pub fn generate_enc_shares(
        &mut self,
        grpkey: Grpkey,
        sk: FieldElement,
        id: usize,
        opener_pubkeys: &[G2],
        threshold: usize,
    ) -> (Vec<EncShare>, Vec<thressig::G1>) {
        create_enc_shares(&id, &grpkey, threshold, opener_pubkeys, &sk)
    }

    pub fn generate_elgamal_key_pair(&mut self) -> Result<(), String> {
        let g_tilde = &self.grpkey.as_ref().ok_or("Grpkey has to be set")?.g_tilde;
        self.elgamal_keypair = Some(thressig::elgamal_keygen(g_tilde));
        Ok(())
    }

    pub fn add_partial_memkey(&mut self, memkey: Vec<u8>) -> Result<(), String> {
        // The sk comes from generate_join_key_material; a member key without it is useless.
        let sk = self.memkey.as_ref().ok_or("no join in progress")?.sk.clone();
        let mut partial_memkey =
            Memkey::from_bytes(&memkey).map_err(|e| format!("member key: {e}"))?;
        partial_memkey.sk = sk;
        self.memkey = Some(partial_memkey);
        Ok(())
    }

    pub fn from_file(path: PathBuf) -> Result<StoredKeyMaterial, Box<dyn std::error::Error>> {
        let key_material_file = OpenOptions::new().read(true).open(path)?;

        let key_material: StoredKeyMaterial = serde_json::from_reader(key_material_file)?;
        // get_keypair and get_peer_id rely on a valid Ed25519 key.
        let private = key_material.signature_key_pair.private();
        if private.len() < 32 || Keypair::ed25519_from_bytes(private[..32].to_vec()).is_err() {
            return Err("the file holds no valid Ed25519 signature key".into());
        }
        Ok(key_material)
    }

    //Sets the grpkey
    pub fn set_grpkey(&mut self, grpkey: Vec<u8>) -> Result<(), String> {
        let grpkey = Grpkey::from_bytes(&grpkey).map_err(|e| format!("group key: {e}"))?;
        self.grpkey = Some(grpkey);
        Ok(())
    }

    //Sets the memkey
    pub fn set_memkey(&mut self, memkey: Vec<u8>) -> Result<(), String> {
        let memkey = Memkey::from_bytes(&memkey).map_err(|e| format!("member key: {e}"))?;
        self.memkey = Some(memkey);
        Ok(())
    }

    //Sets the ID
    pub fn set_id(&mut self, id: usize) {
        self.id = Some(id);
    }

    pub fn to_file(&self, path: PathBuf) -> Result<(), String> {
        let file = File::create(path).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(file, self).map_err(|e| e.to_string())?;
        Ok(())
    }
}
