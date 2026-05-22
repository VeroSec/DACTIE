use std::ops::{Mul, Neg};
use amcl_wrapper::extension_field_gt::GT;
pub use amcl_wrapper::field_elem::FieldElement;
pub use amcl_wrapper::group_elem::GroupElement;
pub use amcl_wrapper::group_elem_g1::G1;
pub use amcl_wrapper::group_elem_g2::G2;
use serde::Serialize;
use thiserror::Error;
use structs::*;
use crate::PS16Error::{MemkeySetError, SerializationError, SignatureInvalid};

pub mod structs;
mod spk_dlog;
mod spk_pairing_homomorphism;
//#[cfg(test)]
//mod tests;

pub const SIGNATURE_LENGTH: usize = 614;

pub fn setup() -> (Grpkey, Mgrkey) {
    let g = G1::random();
    let g_tilde = G2::random();
    let x = FieldElement::random();
    let y = FieldElement::random();
    let x_pub_tilde = &g_tilde * &x;
    let y_pub_tilde = &g_tilde * &y;


    (Grpkey {
        r: FieldElement::new(),
        g,
        g_tilde,
        x_pub_tilde,
        y_pub_tilde,
    }, Mgrkey {
        x,
        y,
    })
}


/**
 * Executes part 1 of the member-side join of the PS16 scheme.
 * Member generates sk and provides ZKP.
 */
pub fn join_mem1(n:&G1, grpkey: &Grpkey) -> (G1, G2, (FieldElement, FieldElement), FieldElement) {

    let sk = FieldElement::random();
    let tau = &grpkey.g * &sk;
    let ttau = &grpkey.y_pub_tilde * &sk;
    let pi = spk_dlog::sign(&tau,&grpkey.g,&n.to_bytes(false),&sk);

    (tau,ttau,pi,sk)
}

/**
 * Executes part 2 of the member-side join of the PS16 scheme.
 * A member combines sk and the partial key
 */
pub fn join_mem2(sk: &FieldElement, partial_memkey: &Memkey) -> Memkey{
    let mut result =partial_memkey.clone();
    result.sk=sk.clone();
    result

}

/**
 * Executes part 1 of the manager-side join of the PS16 scheme.
 * A random number is provided to the member
 */
pub fn join_mgr1() -> G1 {
    G1::random()

}

/**
 * Executes part 2 of the manager-side join of the PS16 scheme.
 * Manager validates Member data and indirectly signs sk if valid
 * Implements the not information theoretical implementation of the aggregate function
 */
pub fn join_mgr2(gml: &mut Vec<(usize,G1, G2)>, mgrkey: &Mgrkey, grpkey: &Grpkey, n:&G1, tau:&G1, ttau:&G2, pi:(FieldElement, FieldElement)) -> Result<(usize,Memkey),String>{
    if spk_dlog::verify(&tau,&grpkey.g,pi,&n.to_bytes(false)) == false {
        return Err(String::from("error"));
    }

    let e1 = GT::ate_pairing(&tau,&grpkey.y_pub_tilde);
    let e2 = GT::ate_pairing(&grpkey.g,&ttau);
    if e1 != e2 {
        return Err(String::from("error"));
    }

    let u = FieldElement::random();
    let sigma1 = &grpkey.g * &u;
    let sigma2 = (tau * &mgrkey.y+(&grpkey.g * &mgrkey.x))*&u;
    let e = GT::ate_pairing(&sigma1,&grpkey.y_pub_tilde);
    let id = gml.len();
    gml.push((id,tau.clone(),ttau.clone()));
    Ok((id,Memkey{
        sk:FieldElement::new(),
        sigma1: Some(sigma1),
        sigma2: Some(sigma2),
        e: Some(e)
    }))
}

/**
 * Signs a message using the member key and provide a ZKP of Knowledge sk.
 **/
pub fn sign<T: Serialize>(memkey: &Memkey, msg: &T) -> Result<Signature, PS16Error>{

    let msg = bincode::serialize(msg).map_err(|_| SerializationError)?;
    let sigma1= &memkey.clone().sigma1.ok_or(MemkeySetError{val:"sigma1".to_string()})?;
    let sigma2= &memkey.clone().sigma2.ok_or(MemkeySetError{val:"sigma2".to_string()})?;
    let e= &memkey.clone().e.ok_or(MemkeySetError{val:"e".to_string()})?;

    /* Randomize sigma1 and sigma2 */
    let t = FieldElement::random();
    let sigma1 = sigma1 * &t;
    let sigma2 = sigma2 * &t;


    let k = FieldElement::random();
    let e = e.pow(&(&k*&t));

    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend(sigma1.to_bytes(false));
    msg_to_send.extend(sigma2.to_bytes(false));
    msg_to_send.extend(e.to_bytes());
    msg_to_send.extend(msg);

    /* c = hash(ps16_sig->sigma1,ps16_sig->sigma2,e,m) */
    let c = FieldElement::from_msg_hash(&msg_to_send);
    let s = (&c * &memkey.sk) + &k;

    Ok(Signature{
        sigma1,
        sigma2,
        c,
        s
    })
}

/**
 * Verifier checks signature and ZKP of sk.
**/
pub fn verify<T: Serialize+ ?Sized>(sig: &Signature, grpkey: &Grpkey, msg: &T) -> Result<(), PS16Error>{
    let msg = bincode::serialize(msg).map_err(|_| SerializationError)?;

    /* e1 = e(sigma1^-1,X) */
    let e1 = GT::ate_pairing(&(&sig.sigma1).neg(),&grpkey.x_pub_tilde);
    /* e2 = e(sigma2,gg) */
    let e2 = GT::ate_pairing(&sig.sigma2,&grpkey.g_tilde);
    /* e3 = e(sigma1^s,Y) */
    let e3 = GT::ate_pairing(&(&sig.sigma1*&sig.s),&grpkey.y_pub_tilde);
    /* r_pub = (e1*e2)^-c*e3 */
    let r_pub = (e1 * e2).pow(&sig.c).inverse()*e3;

    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend((&sig.sigma1).to_bytes(false));
    msg_to_send.extend((&sig.sigma2).to_bytes(false));
    msg_to_send.extend(r_pub.to_bytes());
    msg_to_send.extend(msg);

    /* c = Hash(sigma1,sigma2,R,m) */
    let c = FieldElement::from_msg_hash(&msg_to_send);


    if sig.c == c{
        Ok(())
    } else {
        Err(SignatureInvalid)
    }

}

/**
 * Manager opens signature by comparing signature with stored gml values.
**/
pub fn open(sig: &Signature, grpkey: &Grpkey, gml:&Vec<(usize,G1, G2)>) -> Result<(usize,(FieldElement,G2)),String>{
    let mut e1 = GT::ate_pairing(&sig.sigma2,&grpkey.g_tilde);
    let e2 = GT::ate_pairing(&sig.sigma1,&grpkey.x_pub_tilde);
    e1 = e1.mul(e2.inverse());

    let mut i = 0;
    let (id,e3,ttau) = loop {
        if i >= gml.len() {
            break Err("No match found!")
        }
        let ttau = &gml[i].2;
        let e3 = GT::ate_pairing(&sig.sigma1,ttau);
        if e1 == e3 {
            break Ok((&gml[i].0,e3,ttau));
        }
        i = i + 1;
    }?;
    Ok((*id,spk_pairing_homomorphism::sign(&sig.sigma1,&e3,ttau,&sig.to_bytes())))
}

/**
 * Receiver of opened signature can check if it was opened
 **/
pub fn open_verify(sig: &Signature, grpkey: &Grpkey,pi:(FieldElement,G2)) -> bool{
    let e1 = GT::ate_pairing(&sig.sigma2,&grpkey.g_tilde);
    let e2 = GT::ate_pairing(&sig.sigma1,&grpkey.x_pub_tilde);

    let e = e1.mul(e2.inverse());

    spk_pairing_homomorphism::verify(&sig.sigma1,&e,&pi,&sig.to_bytes())
}

#[derive(Debug, Error)]
pub enum PS16Error {
    #[error("Memkey is not set. Missing value: {val}")]
    MemkeySetError{val:String},
    #[error("Could not serialize Value")]
    SerializationError,
    #[error("Signature is invalid")]
    SignatureInvalid,

}
