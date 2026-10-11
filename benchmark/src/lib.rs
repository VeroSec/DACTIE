//! Fixtures for the thressig microbenchmarks.
//!
//! A [`Group`] is a fully joined DACTIE group: an authority, `n_openers` archives
//! holding ElGamal opening keys, and `n_members` members whose secret keys are
//! Shamir-shared to the archives with threshold `t`. Building it is slow for large
//! groups, so the benchmarks build it once and only time the operation under test.

use std::collections::{HashMap, HashSet};
use thressig::structs::{EncShare, EncSharePairing, Grpkey, Memkey, Mgrkey, Signature};
use thressig::{FieldElement, GT};

pub struct Group {
    pub grpkey: Grpkey,
    pub mgrkey: Mgrkey,
    pub threshold: usize,
    pub opener_pub_keys: Vec<thressig::G2>,
    pub opener_sec_keys: Vec<FieldElement>,
    /// Group membership list kept by the authority (member id -> name).
    pub gml: HashMap<usize, String>,
    pub memkeys: Vec<Memkey>,
    /// `enc_shares[member][archive]`: the share of a member's key encrypted to an archive.
    pub enc_shares: Vec<Vec<EncShare>>,
}

/// Runs the complete join protocol for one member and returns its key and encrypted shares.
pub fn join(
    id: usize,
    grpkey: &Grpkey,
    mgrkey: &Mgrkey,
    threshold: usize,
    opener_pub_keys: &[thressig::G2],
    gml: &mut HashMap<usize, String>,
) -> (Memkey, Vec<EncShare>) {
    let n = thressig::join_mgr1();
    let (g_sk, h_sk, pi1, pi2, sk, enc_shares, commits) =
        thressig::join_mem1(id, n.clone(), grpkey, threshold, opener_pub_keys);
    let partial_memkey = thressig::join_mgr2(
        id,
        thressig::JoinMgr2Args {
            gml,
            name: format!("member-{id}"),
            mgrkey,
            grpkey,
            n: &n,
            g_sk: &g_sk,
            h_sk: &h_sk,
            opener_pubkeys: opener_pub_keys,
            commits: &commits,
            pi1,
            pi2,
            enc_shares: &enc_shares,
        },
    )
    .expect("join_mgr2 rejected an honest member");
    (thressig::join_mem2(&sk, &partial_memkey), enc_shares)
}

impl Group {
    pub fn new(n_members: usize, n_openers: usize, threshold: usize) -> Self {
        assert!(threshold <= n_openers, "threshold must not exceed the number of archives");
        let (grpkey, mgrkey) = thressig::setup();
        let (opener_sec_keys, opener_pub_keys): (Vec<_>, Vec<_>) = (0..n_openers)
            .map(|_| thressig::elgamal_keygen(&grpkey.g_tilde))
            .unzip();
        let mut gml = HashMap::new();
        let mut memkeys = Vec::with_capacity(n_members);
        let mut enc_shares = Vec::with_capacity(n_members);
        for id in 0..n_members {
            let (memkey, shares) =
                join(id, &grpkey, &mgrkey, threshold, &opener_pub_keys, &mut gml);
            memkeys.push(memkey);
            enc_shares.push(shares);
        }
        Group { grpkey, mgrkey, threshold, opener_pub_keys, opener_sec_keys, gml, memkeys, enc_shares }
    }

    pub fn sign(&self, member: usize, msg: &[u8]) -> Signature {
        thressig::sign(&self.memkeys[member], &self.grpkey, &msg).expect("sign failed")
    }

    /// Work of one archive when an opening is requested: since the signer is unknown,
    /// it computes one pairing per member of the group.
    pub fn archive_open_pairings(&self, sig: &Signature, archive: usize) -> Vec<(usize, EncSharePairing)> {
        (0..self.enc_shares.len())
            .map(|id| {
                let share = self.enc_shares[id][archive].clone();
                (id, thressig::create_open_pairing(sig, share, &self.opener_sec_keys[archive]))
            })
            .collect()
    }

    /// Collects the partial openings of the first `threshold` archives in the shape
    /// expected by [`thressig::open`].
    pub fn collect_pairings(&self, sig: &Signature) -> HashMap<usize, Vec<(usize, GT)>> {
        let mut all_sj: HashMap<usize, Vec<(usize, GT)>> = HashMap::new();
        for archive in 0..self.threshold {
            for (id, p) in self.archive_open_pairings(sig, archive) {
                all_sj.entry(p.index).or_default().push((id, p.pairing));
            }
        }
        all_sj
    }

    pub fn member_ids(&self) -> HashSet<usize> {
        self.gml.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_signs_verifies_and_opens() {
        let group = Group::new(5, 3, 3);
        let msg = vec![7u8; 1024];
        let sig = group.sign(3, &msg);
        thressig::verify(&sig, &group.grpkey, &msg.as_slice()).expect("verify failed");
        let opened = thressig::open(&sig, &group.grpkey, &group.member_ids(), &group.collect_pairings(&sig));
        assert_eq!(opened, vec![3]);
    }
}
