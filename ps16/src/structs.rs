use amcl_wrapper::extension_field_gt::GT;
use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem_g1::G1;
use amcl_wrapper::group_elem_g2::G2;
use serde::{Deserialize, Serialize};
use bincode;



#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grpkey {
    pub r: FieldElement,
    pub g: G1,
    pub g_tilde: G2,
    pub x_pub_tilde: G2,
    pub y_pub_tilde: G2,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mgrkey {
    pub x: FieldElement,
    pub y: FieldElement,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memkey {
    pub sk: FieldElement,
    pub sigma1: Option<G1>,
    pub sigma2: Option<G1>,
    pub e: Option<GT>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Signature {
    pub sigma1: G1,
    pub sigma2: G1,
    pub c: FieldElement,
    pub s: FieldElement,
}

impl Signature {
    pub fn to_bytes(&self) -> Vec<u8> {
        // Serialize the struct to bytes using bincode
        bincode::serialize(self).expect("Serialization failed")
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        // Deserialize the bytes back into the struct using bincode
        bincode::deserialize(bytes).expect("Deserialization failed")
    }
}

impl Memkey {
    pub fn to_bytes(&self) -> Vec<u8> {
        // Serialize the struct to bytes using bincode
        bincode::serialize(self).expect("Serialization failed")
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        // Deserialize the bytes back into the struct using bincode
        bincode::deserialize(bytes).expect("Deserialization failed")
    }
}

impl Grpkey {
    pub fn to_bytes(&self) -> Vec<u8> {
        // Serialize the struct to bytes using bincode
        bincode::serialize(self).expect("Serialization failed")
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        // Deserialize the bytes back into the struct using bincode
        bincode::deserialize(bytes).expect("Deserialization failed")
    }


}
