//! Threshold issuance: n issuers (AAs) hold Shamir shares x_i, y1_i of the issuing key (x, y1)
//! with threshold t. Each issuer runs the same checks as a single manager and returns a
//! partial credential sigma2_i = h_sk * y1_i + sigma1 * x_i. Because sigma1 = g^{H0(id)} is
//! the same for every issuer, any t partials interpolate at 0 to
//! sigma2 = h_sk * y1 + sigma1 * x: exactly the credential a single manager holding (x, y1)
//! issues, under the same group key. Signing, verification, slot tags and opening do not
//! change. No issuer alone, and no t - 1 of them, can issue a credential.

use crate::structs::{EncShare, Grpkey, Memkey, Mgrkey};
use crate::{
    check_join_request, compute_lagrange_coefficients_at_zero, h0, verify_memkey, JoinRequest,
};
use amcl_wrapper::extension_field_gt::GT;
use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem::GroupElement;
use amcl_wrapper::group_elem_g1::G1;
use amcl_wrapper::group_elem_g2::G2;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Issuer `index`'s secret share of the issuing key (indices 1..=n). Kept by that issuer only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssuerShare {
    pub index: usize,
    pub x: FieldElement,
    pub y1: FieldElement,
}

/// Issuer `index`'s public share, g~^{x_i} and g~^{y1_i}, with which a member checks that
/// issuer's partial credential. Public, distributed with the group key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerPub {
    pub index: usize,
    pub x_pub_tilde: G2,
    pub y1_pub_tilde: G2,
}

/// Issuer `index`'s part of a member's credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialCredential {
    pub index: usize,
    pub sigma2: G1,
}

/// Splits the issuing key of `setup()` into `n` shares with threshold `t`: the dealer of a
/// trusted setup ceremony runs this once, hands share i to issuer i and then deletes `mgrkey`.
/// Returns the secret shares and the public shares.
pub fn deal_issuer_shares(
    mgrkey: &Mgrkey,
    grpkey: &Grpkey,
    n: usize,
    t: usize,
) -> Result<(Vec<IssuerShare>, Vec<IssuerPub>), String> {
    if t == 0 || t > n {
        return Err(format!("threshold {t} must be between 1 and {n}"));
    }
    let poly_x = random_polynomial(&mgrkey.x, t);
    let poly_y1 = random_polynomial(&mgrkey.y1, t);
    let mut shares = Vec::with_capacity(n);
    let mut pubs = Vec::with_capacity(n);
    for index in 1..=n {
        let x = evaluate(&poly_x, index);
        let y1 = evaluate(&poly_y1, index);
        pubs.push(IssuerPub {
            index,
            x_pub_tilde: &grpkey.g_tilde * &x,
            y1_pub_tilde: &grpkey.g_tilde * &y1,
        });
        shares.push(IssuerShare { index, x, y1 });
    }
    Ok((shares, pubs))
}

/// Coefficients of a random polynomial of degree t - 1 with constant term `secret`.
fn random_polynomial(secret: &FieldElement, t: usize) -> Vec<FieldElement> {
    let mut coefficients = vec![secret.clone()];
    coefficients.extend((1..t).map(|_| FieldElement::random()));
    coefficients
}

fn evaluate(coefficients: &[FieldElement], at: usize) -> FieldElement {
    let z = FieldElement::from(at as u64);
    // Horner's rule, from the highest coefficient down.
    coefficients
        .iter()
        .rev()
        .fold(FieldElement::zero(), |acc, c| acc * &z + c)
}

/// True if the public shares belong to the group key: any `t` of them interpolate to
/// X~ and Y1~. A member checks this once for the issuer set it is given.
pub fn verify_issuer_pubs(pubs: &[IssuerPub], grpkey: &Grpkey, t: usize) -> bool {
    let indices: HashSet<usize> = pubs.iter().map(|p| p.index).collect();
    if t == 0 || indices.len() != pubs.len() || pubs.len() < t || indices.contains(&0) {
        return false;
    }
    // Every window of t consecutive shares must interpolate to the group key; with n shares
    // on a polynomial of degree t - 1, this covers every share.
    (0..=pubs.len() - t).all(|start| {
        let window = &pubs[start..start + t];
        let lambdas = compute_lagrange_coefficients_at_zero(
            &window.iter().map(|p| p.index).collect::<Vec<_>>(),
        );
        let mut x = G2::identity();
        let mut y1 = G2::identity();
        for (p, lambda) in window.iter().zip(&lambdas) {
            x += &p.x_pub_tilde * lambda;
            y1 += &p.y1_pub_tilde * lambda;
        }
        x == grpkey.x_pub_tilde && y1 == grpkey.y1_pub_tilde
    })
}

/// The member's request as each issuer receives it (see `join_mem1`).
pub struct JoinIssuer2Args<'a> {
    pub gml: &'a mut HashMap<usize, String>,
    pub name: String,
    pub share: &'a IssuerShare,
    pub grpkey: &'a Grpkey,
    pub n: &'a G1,
    pub g_sk: &'a G1,
    pub h_sk: &'a G1,
    pub opener_pubkeys: &'a [G2],
    pub commits: &'a [G1],
    pub pi1: (FieldElement, FieldElement),
    pub pi2: (FieldElement, FieldElement),
    pub pi3: (FieldElement, FieldElement),
    pub enc_shares: &'a [EncShare],
}

/// One issuer's side of the join: the checks of `join_mgr2`, then its partial credential.
/// Like `join_mgr2`, it refuses a MemberID already in its `gml`; with quorums of t of n
/// issuers, any two quorums share 2t - n issuers, so a second credential for one MemberID
/// needs more than 2t - n dishonest issuers.
pub fn join_issuer2(id: usize, args: JoinIssuer2Args) -> Result<PartialCredential, String> {
    let JoinIssuer2Args {
        gml,
        name,
        share,
        grpkey,
        n,
        g_sk,
        h_sk,
        opener_pubkeys,
        commits,
        pi1,
        pi2,
        pi3,
        enc_shares,
    } = args;
    if gml.contains_key(&id) {
        return Err(String::from("ID already in use"));
    }
    let sigma1 = check_join_request(
        id,
        &JoinRequest {
            grpkey,
            n,
            g_sk,
            h_sk,
            opener_pubkeys,
            commits,
            pi1: &pi1,
            pi2: &pi2,
            pi3: &pi3,
            enc_shares,
        },
    )?;
    let sigma2 = h_sk * &share.y1 + (&sigma1 * &share.x);
    gml.insert(id, name);
    Ok(PartialCredential {
        index: share.index,
        sigma2,
    })
}

/// True if `partial` is issuer `partial.index`'s credential part on the member's `sk`:
/// e(sigma2_i, g~) = e(sigma1, X~_i Y1~_i^sk). A member checks every partial before it
/// combines them, and can name the issuer of a bad one.
pub fn verify_partial(
    partial: &PartialCredential,
    pubs: &[IssuerPub],
    grpkey: &Grpkey,
    id: usize,
    sk: &FieldElement,
) -> bool {
    let Some(issuer) = pubs.iter().find(|p| p.index == partial.index) else {
        return false;
    };
    let sigma1 = &grpkey.g * h0(&id);
    let x_y_sk = &issuer.x_pub_tilde + &(&issuer.y1_pub_tilde * sk);
    GT::ate_pairing(&partial.sigma2, &grpkey.g_tilde) == GT::ate_pairing(&sigma1, &x_y_sk)
}

/// Combines `t` partial credentials of distinct issuers into the member's key. Checks every
/// partial first and fails if fewer than `t` are valid; the result passes `verify_memkey`.
pub fn combine_partials(
    partials: &[PartialCredential],
    pubs: &[IssuerPub],
    t: usize,
    grpkey: &Grpkey,
    id: usize,
    sk: &FieldElement,
) -> Result<Memkey, String> {
    let mut seen = HashSet::new();
    let valid: Vec<&PartialCredential> = partials
        .iter()
        .filter(|p| seen.insert(p.index) && verify_partial(p, pubs, grpkey, id, sk))
        .take(t)
        .collect();
    if t == 0 || valid.len() < t {
        return Err(format!(
            "{} valid partial credentials of distinct issuers, {t} needed",
            valid.len()
        ));
    }
    let lambdas =
        compute_lagrange_coefficients_at_zero(&valid.iter().map(|p| p.index).collect::<Vec<_>>());
    let mut sigma2 = G1::identity();
    for (p, lambda) in valid.iter().zip(&lambdas) {
        sigma2 += &p.sigma2 * lambda;
    }
    let sigma1 = &grpkey.g * h0(&id);
    let memkey = Memkey {
        sk: sk.clone(),
        e1: Some(GT::ate_pairing(&sigma1, &grpkey.y1_pub_tilde)),
        sigma1: Some(sigma1),
        sigma2: Some(sigma2),
    };
    if !verify_memkey(&memkey, grpkey, id) {
        return Err(String::from("the combined credential does not verify"));
    }
    Ok(memkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        create_open_pairing, elgamal_keygen, join_mem1, join_mgr1, join_mgr2, open, setup, sign,
        verify, JoinMgr2Args,
    };

    struct Joined {
        grpkey: Grpkey,
        mgrkey: Mgrkey,
        shares: Vec<IssuerShare>,
        pubs: Vec<IssuerPub>,
        partials: Vec<PartialCredential>,
        sk: FieldElement,
        id: usize,
        opener_secret_keys: Vec<FieldElement>,
        enc_shares: Vec<EncShare>,
    }

    /// A member joins with each of `n` issuers (threshold `t`), next to 3 archives (threshold 2).
    fn join_with_issuers(n: usize, t: usize) -> Joined {
        let (grpkey, mgrkey) = setup();
        let (shares, pubs) = deal_issuer_shares(&mgrkey, &grpkey, n, t).unwrap();
        let (opener_secret_keys, opener_pubkeys): (Vec<_>, Vec<_>) =
            (0..3).map(|_| elgamal_keygen(&grpkey.g_tilde)).unzip();
        let nonce = join_mgr1();
        let id = 7usize;
        let (g_sk, h_sk, pi1, pi2, pi3, sk, enc_shares, commits) =
            join_mem1(id, nonce.clone(), &grpkey, 2, &opener_pubkeys);
        let partials = shares
            .iter()
            .map(|share| {
                join_issuer2(
                    id,
                    JoinIssuer2Args {
                        gml: &mut HashMap::new(),
                        name: "member".to_string(),
                        share,
                        grpkey: &grpkey,
                        n: &nonce,
                        g_sk: &g_sk,
                        h_sk: &h_sk,
                        opener_pubkeys: &opener_pubkeys,
                        commits: &commits,
                        pi1: pi1.clone(),
                        pi2: pi2.clone(),
                        pi3: pi3.clone(),
                        enc_shares: &enc_shares,
                    },
                )
                .unwrap()
            })
            .collect();
        Joined {
            grpkey,
            mgrkey,
            shares,
            pubs,
            partials,
            sk,
            id,
            opener_secret_keys,
            enc_shares,
        }
    }

    #[test]
    fn deal_rejects_bad_thresholds() {
        let (grpkey, mgrkey) = setup();
        assert!(deal_issuer_shares(&mgrkey, &grpkey, 3, 0).is_err());
        assert!(deal_issuer_shares(&mgrkey, &grpkey, 3, 4).is_err());
        assert!(deal_issuer_shares(&mgrkey, &grpkey, 3, 3).is_ok());
    }

    #[test]
    fn public_shares_match_the_group_key() {
        let j = join_with_issuers(3, 2);
        assert!(verify_issuer_pubs(&j.pubs, &j.grpkey, 2));
        let (other, _) = setup();
        assert!(!verify_issuer_pubs(&j.pubs, &other, 2));
        let mut swapped = j.pubs.clone();
        swapped[1].x_pub_tilde = swapped[2].x_pub_tilde.clone();
        assert!(!verify_issuer_pubs(&swapped, &j.grpkey, 2));
        assert!(!verify_issuer_pubs(&j.pubs[..1], &j.grpkey, 2));
    }

    #[test]
    fn any_two_of_three_give_the_managers_credential() {
        let j = join_with_issuers(3, 2);
        for pair in [[0, 1], [0, 2], [1, 2]] {
            let chosen: Vec<_> = pair.iter().map(|&i| j.partials[i].clone()).collect();
            let memkey = combine_partials(&chosen, &j.pubs, 2, &j.grpkey, j.id, &j.sk).unwrap();
            assert!(verify_memkey(&memkey, &j.grpkey, j.id));
            // sigma2 = h_sk * y1 + sigma1 * x, as a single manager with the whole key computes it.
            let sigma1 = &j.grpkey.g * h0(&j.id);
            let h_sk = &sigma1 * &j.sk;
            let expected = &h_sk * &j.mgrkey.y1 + (&sigma1 * &j.mgrkey.x);
            assert_eq!(memkey.sigma2, Some(expected));
        }
    }

    #[test]
    fn one_partial_is_not_enough() {
        let j = join_with_issuers(3, 2);
        assert!(combine_partials(&j.partials[..1], &j.pubs, 2, &j.grpkey, j.id, &j.sk).is_err());
        // The same issuer twice does not count as two.
        let twice = vec![j.partials[0].clone(), j.partials[0].clone()];
        assert!(combine_partials(&twice, &j.pubs, 2, &j.grpkey, j.id, &j.sk).is_err());
        // Interpolating one partial as if it were the key gives no valid credential.
        let alone = Memkey {
            sk: j.sk.clone(),
            sigma1: Some(&j.grpkey.g * h0(&j.id)),
            sigma2: Some(j.partials[0].sigma2.clone()),
            e1: Some(GT::ate_pairing(
                &(&j.grpkey.g * h0(&j.id)),
                &j.grpkey.y1_pub_tilde,
            )),
        };
        assert!(!verify_memkey(&alone, &j.grpkey, j.id));
    }

    #[test]
    fn a_bad_partial_is_named_and_skipped() {
        let j = join_with_issuers(3, 2);
        let mut bad = j.partials[0].clone();
        bad.sigma2 = &bad.sigma2 + &j.grpkey.g;
        assert!(!verify_partial(&bad, &j.pubs, &j.grpkey, j.id, &j.sk));
        assert!(verify_partial(
            &j.partials[1],
            &j.pubs,
            &j.grpkey,
            j.id,
            &j.sk
        ));
        // A bad partial plus one good one is not enough ...
        let one_good = vec![bad.clone(), j.partials[1].clone()];
        assert!(combine_partials(&one_good, &j.pubs, 2, &j.grpkey, j.id, &j.sk).is_err());
        // ... but the bad one is skipped when two good ones are there.
        let two_good = vec![bad, j.partials[1].clone(), j.partials[2].clone()];
        assert!(combine_partials(&two_good, &j.pubs, 2, &j.grpkey, j.id, &j.sk).is_ok());
    }

    #[test]
    fn an_issuer_refuses_a_member_id_twice() {
        let j = join_with_issuers(3, 2);
        let mut gml = HashMap::from([(j.id, "earlier".to_string())]);
        let nonce = join_mgr1();
        let (_, pks): (Vec<_>, Vec<_>) = (0..3).map(|_| elgamal_keygen(&j.grpkey.g_tilde)).unzip();
        let (g_sk, h_sk, pi1, pi2, pi3, _, enc_shares, commits) =
            join_mem1(j.id, nonce.clone(), &j.grpkey, 2, &pks);
        let result = join_issuer2(
            j.id,
            JoinIssuer2Args {
                gml: &mut gml,
                name: "again".to_string(),
                share: &j.shares[0],
                grpkey: &j.grpkey,
                n: &nonce,
                g_sk: &g_sk,
                h_sk: &h_sk,
                opener_pubkeys: &pks,
                commits: &commits,
                pi1,
                pi2,
                pi3,
                enc_shares: &enc_shares,
            },
        );
        assert_eq!(result, Err(String::from("ID already in use")));
    }

    #[test]
    fn signing_and_opening_work_as_before() {
        let j = join_with_issuers(3, 2);
        let memkey = combine_partials(&j.partials, &j.pubs, 2, &j.grpkey, j.id, &j.sk).unwrap();
        let signature = sign(&memkey, &j.grpkey, &"report".as_bytes()).unwrap();
        assert!(verify(&signature, &j.grpkey, "report".as_bytes()).is_ok());
        // Two of the three archives open it to the member.
        let mut pairings = HashMap::new();
        for a in [0, 2] {
            let p = create_open_pairing(
                &signature,
                j.enc_shares[a].clone(),
                &j.opener_secret_keys[a],
            );
            pairings.insert(p.index, vec![(j.id, p.pairing)]);
        }
        let ids = open(&signature, &j.grpkey, &HashSet::from([j.id]), &pairings);
        assert_eq!(ids, vec![j.id]);
    }

    #[test]
    fn a_single_manager_still_works() {
        // join_mgr2 shares its checks with join_issuer2; the single-manager path is unchanged.
        let j = join_with_issuers(3, 2);
        let nonce = join_mgr1();
        let (_, pks): (Vec<_>, Vec<_>) = (0..3).map(|_| elgamal_keygen(&j.grpkey.g_tilde)).unzip();
        let (g_sk, h_sk, pi1, pi2, pi3, sk, enc_shares, commits) =
            join_mem1(9, nonce.clone(), &j.grpkey, 2, &pks);
        let partial = join_mgr2(
            9,
            JoinMgr2Args {
                gml: &mut HashMap::new(),
                name: "solo".to_string(),
                mgrkey: &j.mgrkey,
                grpkey: &j.grpkey,
                n: &nonce,
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
        let memkey = crate::join_mem2(&sk, &partial);
        assert!(verify_memkey(&memkey, &j.grpkey, 9));
    }
}
