use amcl_wrapper::extension_field_gt::GT;
use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem::GroupElement;
use amcl_wrapper::group_elem_g1::G1;
use amcl_wrapper::group_elem_g2::G2;
use base64::{engine::general_purpose, Engine as _};
use bincode;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// amcl_wrapper 0.4.0 serializes G1 and FieldElement as hex strings via to_hex(),
// which triples their size on the wire. These adapters use the compact byte form
// instead. Changing them changes the wire format: every peer must agree.
mod g1_compressed {
    use super::*;
    use serde::de::Error as DeError;
    pub fn serialize<S: serde::Serializer>(g: &G1, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&g.to_bytes(true))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<G1, D::Error> {
        let v: Vec<u8> = Vec::deserialize(d)?;
        G1::from_bytes(&v).map_err(|e| D::Error::custom(format!("G1: {:?}", e)))
    }
}

mod fr_bytes {
    use super::*;
    use serde::de::Error as DeError;
    pub fn serialize<S: serde::Serializer>(f: &FieldElement, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&f.to_bytes())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<FieldElement, D::Error> {
        let v: Vec<u8> = Vec::deserialize(d)?;
        FieldElement::from_bytes(&v).map_err(|e| D::Error::custom(format!("Fr: {:?}", e)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grpkey {
    pub r: FieldElement,
    pub g: G1,
    pub g_tilde: G2,
    pub x_pub_tilde: G2,
    pub y1_pub_tilde: G2,
    pub y2_pub_tilde: G2,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mgrkey {
    pub x: FieldElement,
    pub y1: FieldElement,
    pub y2: FieldElement,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memkey {
    pub sk: FieldElement,
    pub sigma1: Option<G1>,
    pub sigma2: Option<G1>,
    pub e1: Option<GT>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Signature {
    #[serde(with = "g1_compressed")]
    pub sigma1: G1,
    #[serde(with = "g1_compressed")]
    pub sigma2: G1,
    #[serde(with = "fr_bytes")]
    pub c: FieldElement,
    #[serde(with = "fr_bytes")]
    pub s: FieldElement,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Share {
    pub index: usize,
    pub share: FieldElement,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncShare {
    pub index: usize,
    pub c0: G2,          //Enc Share Part 1
    pub c1: G2,          //Enc Share Part 2
    pub c: FieldElement, //NIZK Proof challange
    pub s: FieldElement, //NIZK Proof response
    pub h_blind: G1,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncSharePairing {
    pub index: usize,
    pub pairing: GT,
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.to_bytes();
        let encoded = general_purpose::STANDARD.encode(&bytes);
        write!(f, "{encoded}")
    }
}

impl FromStr for Signature {
    type Err = Box<dyn std::error::Error + Send + Sync>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = general_purpose::STANDARD.decode(s)?;
        Ok(Signature::from_bytes(&bytes))
    }
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

impl EncShare {
    pub fn to_bytes(&self) -> Vec<u8> {
        // Serialize the struct to bytes using bincode
        bincode::serialize(self).expect("Serialization failed")
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        // Deserialize the bytes back into the struct using bincode
        bincode::deserialize(bytes).expect("Deserialization failed")
    }
}
