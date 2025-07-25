use std::collections::{HashMap, HashSet};
use std::ops::Neg;
pub use amcl_wrapper::extension_field_gt::GT;
pub use amcl_wrapper::field_elem::FieldElement;
pub use amcl_wrapper::group_elem::GroupElement;
pub use amcl_wrapper::group_elem_g1::G1;
pub use amcl_wrapper::group_elem_g2::G2;
use serde::Serialize;
use thiserror::Error;
use structs::*;
use crate::ThressigError::{MemkeySetError, SerializationError, SignatureInvalid};
use sha2::{Sha256, Digest};
pub mod structs;
mod spk_dlog;

#[cfg(test)]
mod tests;

pub fn setup() -> (Grpkey, Mgrkey) {
    let g = G1::random();
    let g_tilde = G2::random();
    let x = FieldElement::random();
    let y1 = FieldElement::random();
    let y2 = FieldElement::random();
    let x_pub_tilde = &g_tilde * &x;
    let y1_pub_tilde = &g_tilde * &y1;
    let y2_pub_tilde = &g_tilde * &y2;


    (Grpkey {
        r: FieldElement::new(),
        g,
        g_tilde,
        x_pub_tilde,
        y1_pub_tilde,
        y2_pub_tilde,
    }, Mgrkey {
        x,
        y1,
        y2,
    })
}


fn hash_to_field(tag: &[u8], id: &usize) -> FieldElement {
    let mut hasher = Sha256::new();
    hasher.update(tag); // domain separation
    hasher.update(id.to_be_bytes());
    let digest = hasher.finalize();

    // Convert hash output to field element mod r
    FieldElement::from_msg_hash(&digest)
}

fn h0(id: &usize) -> FieldElement {
    let h = hash_to_field(b"H0-h|", id);
    h
}

pub fn shamir_feldman_share(
    sk_id: &FieldElement,
    t: usize,
    n: usize,
    h: &G1,
) -> (Vec<Share>, Vec<G1>) {
    // 1. Generate random polynomial coefficients: p_1 to p_t
    let mut coeffs: Vec<FieldElement> = vec![sk_id.clone()];
    for _ in 0..(t-1) {
        coeffs.push(FieldElement::random());
    }

    // 2. Compute shares s_i = P(i)
    let mut shares = vec![];
    for i in 1..=n {
        let x = FieldElement::from(i as u64);
        let mut y = FieldElement::zero();
        for (j, coeff) in coeffs.iter().enumerate() {
            let xj = x.pow(&FieldElement::from(j as u64));
            y = y + coeff * xj;
        }
        shares.push(Share{index:i,share:y});
    }

    // 3. Compute Feldman commitments h_j = h^{p_j}
    let commitments: Vec<G1> = coeffs.iter().map(|c| h * c).collect();

    (shares, commitments)
}

pub fn recover_secret(shares: &[Share]) -> FieldElement {
    let mut secret = FieldElement::zero();

    for Share{index:xj,share:yj} in shares {
        let mut num = FieldElement::one();
        let mut den = FieldElement::one();

        for Share{index:xm,..} in shares {
            if xm == xj {
                continue;
            }
            let xm_fe = FieldElement::from(xm.clone() as u64);
            let xj_fe = FieldElement::from(xj.clone() as u64);

            num = num * (&xm_fe).neg();               // num *= -xm
            den = den * (&xj_fe - &xm_fe);           // den *= (xj - xm)
        }

        let lagrange_coeff = num * den.inverse(); // λj
        secret += yj * &lagrange_coeff;
    }

    secret
}

pub fn verify_share(
    h: &G1,                            // Generator used for commitments
    share: Share,       // (i, s_i)
    commitments: &[G1],               // Feldman commitments h_j = h^{p_j}
) -> bool {
    let Share{index,share} = share;

    // Compute LHS: h^{s_i}
    let lhs = h * share;

    // Compute RHS: ∏_{j=0}^{t} h_j^{i^j}
    let mut rhs = G1::identity(); // Identity element in G1
    let mut i_pow = FieldElement::one(); // i^0

    for h_j in commitments {
        rhs += h_j * &i_pow;
        i_pow = i_pow *  FieldElement::from(index as u64); // i^j for next round
    }

    lhs == rhs
}

pub fn elgamal_keygen(g_tilde: &G2) -> (FieldElement, G2) {
    let sk = FieldElement::random(); // Secret key in Zp
    let pk = g_tilde * &sk;                // Public key: g^sk in G1
    (sk, pk)
}

pub fn create_enc_shares(id: &usize, grpkey: &Grpkey, threshold: usize,opener_pubkeys: &[G2], sk: &FieldElement) -> (Vec<EncShare>, Vec<G1>) {
    let alpha = FieldElement::random();
    let h_blind = &grpkey.g * h0(id) * &alpha;

    let (shares,commits) = shamir_feldman_share(&sk,threshold,opener_pubkeys.len(),&h_blind);

    let mut enc_shares = Vec::new();
    for (i,s) in shares.iter().enumerate(){
        let enc_share = prove_enc_share(&grpkey,&s,&opener_pubkeys[i],&h_blind,&commits);
        enc_shares.push(enc_share);
    };
    (enc_shares,commits)
}

//Creates encrypted shares and blinds commits
pub fn prove_enc_share(
    grpkey: &Grpkey,
    share: &Share,
    f_i: &G2,
    h: &G1,
    h_coms: &[G1],
) -> EncShare {
    //ElGamal Encryption of Share with f_i(key of Archive)
    let r = FieldElement::random();
    let c0 = &grpkey.g_tilde * &r;
    let c1 = (f_i * &r) + (&grpkey.y1_pub_tilde * &share.share);



    //Create necessary values for NI Schnorr Proof
    let k = FieldElement::random();
    let t0 = &grpkey.g_tilde * &k;
    // Compute pairing commitment
    let t1 = GT::ate_pairing(&h, f_i).pow(&k); // e(h,f^k)
    

    // Fiat Shamir Hash challenge
    let mut hash_input = vec![];
    hash_input.extend(&grpkey.g_tilde.to_bytes(false));
    hash_input.extend(f_i.to_bytes(false));
    hash_input.extend(&grpkey.y1_pub_tilde.to_bytes(false));
    hash_input.extend(h.to_bytes(false));
    for h_l in h_coms {
        hash_input.extend(h_l.to_bytes(false));
    }
    hash_input.extend(&share.index.to_be_bytes());
    hash_input.extend(c0.to_bytes(false));
    hash_input.extend(c1.to_bytes(false));
    hash_input.extend(t0.to_bytes(false));
    hash_input.extend(t1.to_bytes());

    let c = FieldElement::from_msg_hash(&hash_input);
    let s = &k + &(&c * r);

    EncShare{index:share.index,c0,c1,c,s,h_blind:h.clone()}
}

pub fn verify_enc_share_correctness(
    EncShare{index, c0,c1,c,s,h_blind}: &EncShare,
    groupkey: &Grpkey,
    f_i: &G2,
    h_coms: &[G1],
) -> bool {
    //Recompute t0 without knowledge of r
    let t0 = &groupkey.g_tilde * s - c0 * c;

    // Compute the verification value of from the commits RHS: ∏_{j=0}^{t} h_j^{i^j}
    let mut rhs = G1::identity(); // Identity element in G1
    let mut i_pow = FieldElement::one(); // i^0

    for h_j in h_coms {
        rhs += h_j * &i_pow;
        i_pow = i_pow *  FieldElement::from(*index as u64); // i^j for next round
    }

    //Use the verification value to recompute t1 without knowledge of r_i and s_i
    let t1 = GT::ate_pairing(h_blind, &(f_i*s)) * GT::ate_pairing(&(h_blind*-c), c1) * GT::ate_pairing(&rhs, &(&groupkey.y1_pub_tilde*c));


    // Recompute Hash based on own calculated values
    let mut hash_input = vec![];
    hash_input.extend(&groupkey.g_tilde.to_bytes(false));
    hash_input.extend(f_i.to_bytes(false));
    hash_input.extend(&groupkey.y1_pub_tilde.to_bytes(false));
    hash_input.extend(h_blind.to_bytes(false));
    for h_l in h_coms {
        hash_input.extend(h_l.to_bytes(false));
    }
    hash_input.extend(&index.to_be_bytes());
    hash_input.extend(c0.to_bytes(false));
    hash_input.extend(c1.to_bytes(false));
    hash_input.extend(t0.to_bytes(false));
    hash_input.extend(t1.to_bytes());

    let c_check = FieldElement::from_msg_hash(&hash_input);

    //Check if hash is correct
    *c == c_check
}





/**
 * Executes part 1 of the member-side join of the PS16 scheme.
 * Member generates sk and provides ZKP.
 */
pub fn join_mem1(id:usize, n:G1, grpkey: &Grpkey, threshold: usize,opener_pubkeys: &[G2]) -> (G1, G1, (FieldElement, FieldElement), (FieldElement, FieldElement), FieldElement, Vec<EncShare>, Vec<G1>) {
    let sk = FieldElement::random();
    let g_sk = &grpkey.g * &sk;
    let h = &grpkey.g * h0(&id);

    let (enc_shares, commits) = create_enc_shares(&id,&grpkey,threshold, opener_pubkeys,&sk);

    let h_sk = &h * &sk;
    let pi1 = spk_dlog::sign(&g_sk,&grpkey.g,&n.to_bytes(false),&sk);
    let pi2 = spk_dlog::sign(&h_sk,&h,&n.to_bytes(false),&sk);


    (g_sk,h_sk,pi1,pi2,sk,enc_shares,commits)
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
pub fn join_mgr2(id: usize, gml: &mut HashMap<usize,String>, name: String,mgrkey: &Mgrkey, grpkey: &Grpkey, n:&G1, g_sk:&G1, h_sk:&G1, opener_pubkeys: &[G2], commits: &[G1],pi1:(FieldElement, FieldElement),pi2:(FieldElement, FieldElement),enc_shares:&[EncShare]) -> Result<Memkey,String>{
    //Check if ID is already in gml
    if gml.contains_key(&id) {
        return Err(String::from("ID already in use"));
    }

    let h = &grpkey.g * h0(&id);

    if spk_dlog::verify(&g_sk,&grpkey.g,pi1,&n.to_bytes(false)) == false {
        return Err(String::from("error"));
    }

    if spk_dlog::verify(&h_sk,&h,pi2,&n.to_bytes(false)) == false {
        return Err(String::from("error"));
    }

    for (i,enc_share) in enc_shares.iter().enumerate() {
        if verify_enc_share_correctness(enc_share, grpkey, &opener_pubkeys[i],commits) == false {
            return Err(String::from("Could not verify share"));
        }
    }

    
    let sigma1 = h;
    let sigma2 = h_sk  * &mgrkey.y1+(&sigma1 * &mgrkey.x);
    let e1 = GT::ate_pairing(&sigma1,&grpkey.y1_pub_tilde);
    gml.insert(id,name);
    Ok(Memkey{
        sk:FieldElement::new(),
        sigma1: Some(sigma1),
        sigma2: Some(sigma2),
        e1: Some(e1),
    })
}

/**
 * Signs a message using the member key and provide a ZKP of Knowledge sk.
 **/
pub fn sign<T: Serialize>(memkey: &Memkey, grpkey: &Grpkey, msg: &T) -> Result<Signature, ThressigError>{

    let msg = bincode::serialize(msg).map_err(|_| SerializationError)?;
    let sigma1= &memkey.clone().sigma1.ok_or(MemkeySetError{val:"sigma1".to_string()})?;
    let sigma2= &memkey.clone().sigma2.ok_or(MemkeySetError{val:"sigma2".to_string()})?;
    let e1= &memkey.clone().e1.ok_or(MemkeySetError{val:"e1".to_string()})?;

    /* Randomize sigma1 and sigma2 */
    let t = FieldElement::random();
    let sigma1 = sigma1 * &t;
    let sigma2 = sigma2 * &t;

    let k1 = FieldElement::random();
    let e = e1.pow(&(&k1 * &t));

    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend(grpkey.to_bytes());
    msg_to_send.extend(sigma1.to_bytes(false));
    msg_to_send.extend(sigma2.to_bytes(false));
    msg_to_send.extend(e.to_bytes());
    msg_to_send.extend(msg);

    /* c = hash(ipk,ps16_sig->sigma1,ps16_sig->sigma2,e,m) */ 
    let c = FieldElement::from_msg_hash(&msg_to_send);
    let s = (&c * &memkey.sk) + &k1;

    Ok(Signature{
        sigma1,
        sigma2,
        c,
        s,
    })
}

/**
 * Verifier checks signature and ZKP of sk.
**/
pub fn verify<T: Serialize+ ?Sized>(sig: &Signature, grpkey: &Grpkey, msg: &T) -> Result<(), ThressigError>{
    let msg = bincode::serialize(msg).map_err(|_| SerializationError)?;

    /* e1 = e(sigma1^-1,X) */
    let e1 = GT::ate_pairing(&(&sig.sigma1).neg(),&grpkey.x_pub_tilde);
    /* e2 = e(sigma2,gg) */
    let e2 = GT::ate_pairing(&sig.sigma2,&grpkey.g_tilde);
    /* e3 = e(sigma1^s1,Y1) */
    let e3 = GT::ate_pairing(&(&sig.sigma1*&sig.s),&grpkey.y1_pub_tilde);
    /* r_pub = (e1*e2)^-c*e3 */
    let r_pub = (e1 * e2).pow(&sig.c).inverse()*e3;

    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend(grpkey.to_bytes());
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

pub fn create_open_pairing(sig: &Signature,enc_share: EncShare,open_sk: &FieldElement) -> EncSharePairing{
    EncSharePairing{index: enc_share.index, pairing: GT::ate_pairing(&sig.sigma1, &(&enc_share.c1-(&enc_share.c0*open_sk)))}
}

/// Compute Lagrange coefficients for interpolation at 0
pub fn compute_lagrange_coefficients_at_zero(shares: &[usize]) -> Vec<FieldElement> {
    let mut coeffs = Vec::new();

    for xj in shares {
        let mut num = FieldElement::one();
        let mut den = FieldElement::one();

        for xm in shares {
            if xm == xj {
                continue;
            }
            let xm_fe = FieldElement::from(*xm as u64);
            let xj_fe = FieldElement::from(*xj as u64);
            
            num = num * (&xm_fe).neg();               // num *= -xm
            den = den * (&xj_fe - &xm_fe);           // den *= (xj - xm)
        }

        let lambda_j = num * den.inverse(); // λ_j
        coeffs.push(lambda_j);
    }

    coeffs
}

/**
 * Manager opens signature by comparing signature with stored gml values.
**/
pub fn open<K>(sig: &Signature, grpkey: &Grpkey, gml: &HashSet<K>, all_sj: &HashMap<usize, Vec<(K, GT)>>) -> Vec<K>
    where
        K: Eq+Clone,
{
    let mut res = Vec::new();
    let mut opener_ids: Vec<_> = all_sj.keys().copied().collect();
    opener_ids.sort();
    for id in gml.iter() {
        let mut t_idj = Vec::new();
        let mut missing = false;
        for j in &opener_ids {
            if let Some(sj_entries) = all_sj.get(&(*j)) {
                if let Some((_, t)) = sj_entries.iter().find(|(uid, _)| uid == id) {
                    t_idj.push(t.clone());
                } else {
                    missing = true;
                    break;
                }
            } else {
                missing = true;
                break;
            }
        }
        if missing {
            continue;
        }


        // Compute the pairing left-hand side
        let pairing_left = GT::ate_pairing(&sig.sigma1, &(&grpkey.x_pub_tilde));

        let w_js = compute_lagrange_coefficients_at_zero(&opener_ids);

        let mut t_combined = GT::one();
        for (t, w) in t_idj.iter().zip(w_js.iter()) {
            t_combined = t_combined *  t.pow(w);
        }
        

        let pairing_right = GT::ate_pairing(&sig.sigma2, &grpkey.g_tilde);
        if pairing_left * t_combined == pairing_right {
            res.push(id.clone());
        }

    }

    res
}

#[derive(Debug, Error)]
pub enum ThressigError {
    #[error("Memkey is not set. Missing value: {val}")]
    MemkeySetError{val:String},
    #[error("Could not serialize Value")]
    SerializationError,
    #[error("Signature is invalid")]
    SignatureInvalid,

}


/*
pub fn prove_correct_encryption(
    r_i: &FieldElement,
    g_tilde: &G2,
    f_i: &G2,
    h: &G1,
    c0: &G2,
    c1: &G2,
    hsk: &G1,
    hs: &[G1],          // h_1, ..., h_t
    i: u64,
    y0: &G2,
) -> (G2, GT, FieldElement) {
    // 1. Compute commitment
    let w = FieldElement::random();

    let a0 = g_tilde * &w;        // a0 = g̃^w
    let f_i_w = f_i * &w;
    let a1 = GT::ate_pairing(h, &(c1 - &f_i_w)); // e(h, c1 / f_i^w)

    // 2. Compute commitment RHS value
    let mut expected = hsk.clone();
    let i_fe = FieldElement::from(i);
    let mut i_pow = FieldElement::one();
    for h_l in hs {
        i_pow = i_pow * &i_fe;
        expected += h_l * &i_pow;
    }
    let rhs = GT::ate_pairing(&expected, y0); // e(hsk * ∏ h_l^{i^l}, Ỹ₀)

    let a1 = a1 * rhs.inverse();

    // 3. Fiat-Shamir challenge: H(g̃, f_i, c0, c1, a0, a1)
    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend(g_tilde.to_bytes(false));
    msg_to_send.extend(f_i.to_bytes(false));
    msg_to_send.extend(c0.to_bytes(false));
    msg_to_send.extend(c1.to_bytes(false));
    msg_to_send.extend(a0.to_bytes(false));
    msg_to_send.extend(a1.to_bytes());

    let c = FieldElement::from_msg_hash(&msg_to_send);



    let z = w + c * r_i;

    ( a0, a1, z )
}*/


/*
pub fn verify_correct_encryption(
    a0: &G2,
    a1: &GT,
    z: &FieldElement,
    g_tilde: &G2,
    f_i: &G2,
    h: &G1,
    c0: &G2,
    c1: &G2,
    hsk: &G1,
    hs: &[G1],
    i: u64,
    y0: &G2,
) -> bool {
    // 1. Recompute Fiat-Shamir challenge
    let mut msg_to_hash: Vec<u8> = Vec::new();
    msg_to_hash.extend(g_tilde.to_bytes(false));
    msg_to_hash.extend(f_i.to_bytes(false));
    msg_to_hash.extend(c0.to_bytes(false));
    msg_to_hash.extend(c1.to_bytes(false));
    msg_to_hash.extend(a0.to_bytes(false));
    msg_to_hash.extend(a1.to_bytes());

    let c = FieldElement::from_msg_hash(&msg_to_hash);

    // 2. Check a0 * c0^c == g_tilde^z
    let lhs_g2 = g_tilde * z;
    let rhs_g2 = a0 + c0 * &c;

    if lhs_g2 != rhs_g2 {
        return false;
    }

    // 3. Recompute expected pairing in GT
    let f_i_z = f_i * z;
    let c1_div = c1 - &f_i_z;
    let lhs_gt = GT::ate_pairing(h, &c1_div);

    let mut expected = hsk.clone();
    let i_fe = FieldElement::from(i);
    let mut i_pow = FieldElement::one();
    for h_l in hs {
        i_pow = i_pow * &i_fe;
        expected += h_l * &i_pow;
    }
    let rhs_gt = GT::ate_pairing(&expected, y0) * a1;

    lhs_gt == rhs_gt
}

 */