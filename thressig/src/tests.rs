use super::*;

#[cfg(test)]
#[test]
fn test_setup() {
    let (grpkey, mgrkey) = setup();
    let (grpkey2, mgrkey2) = setup();
    assert_eq!(&(&mgrkey.x * &grpkey.g_tilde), &grpkey.x_pub_tilde);
    assert_eq!(&(&mgrkey.y1 * &grpkey.g_tilde), &grpkey.y1_pub_tilde);
    assert_eq!(&(&mgrkey.y2 * &grpkey.g_tilde), &grpkey.y2_pub_tilde);
    assert_ne!(&grpkey.g, &grpkey2.g);
    assert_ne!(&grpkey.g_tilde, &grpkey2.g_tilde);
    assert_ne!(mgrkey.x, mgrkey2.x);
    assert_ne!(mgrkey.y1, mgrkey2.y1);
}

#[test]
fn test_flow() {
    let mut gml: HashMap<usize, String> = HashMap::new();
    let (grpkey, mgrkey) = setup();
    let mut opener_pub_keys = Vec::new();
    let mut opener_sec_keys = Vec::new();
    for _ in 1..=6 {
        let (sk, pk) = elgamal_keygen(&grpkey.g_tilde);
        opener_pub_keys.push(pk);
        opener_sec_keys.push(sk);
    }
    let n = join_mgr1();
    let id = 0usize;
    let (g_sk, h_sk, pi1, pi2, pi3, sk, enc_shares, commits) =
        join_mem1(id, n.clone(), &grpkey, 3, &opener_pub_keys);
    let partial_memkey = join_mgr2(
        id,
        JoinMgr2Args {
            gml: &mut gml,
            name: "test".to_string(),
            mgrkey: &mgrkey,
            grpkey: &grpkey,
            n: &n,
            g_sk: &g_sk,
            h_sk: &h_sk,
            opener_pubkeys: &opener_pub_keys,
            commits: &commits,
            pi1,
            pi2,
            pi3,
            enc_shares: &enc_shares,
        },
    )
    .unwrap();
    let memkey1 = join_mem2(&sk, &partial_memkey);
    let y = sign(&memkey1, &grpkey, &"Hallo".as_bytes()).unwrap();

    //Simuluate 3 Openers who work together
    let paring_0 = &create_open_pairing(&y, enc_shares[0].clone(), &opener_sec_keys[0]);
    let paring_1 = &create_open_pairing(&y, enc_shares[1].clone(), &opener_sec_keys[1]);
    let paring_2 = &create_open_pairing(&y, enc_shares[2].clone(), &opener_sec_keys[2]);

    let mut opener_map = HashMap::new();
    opener_map.insert(paring_0.index, vec![(id, paring_0.pairing.clone())]);
    opener_map.insert(paring_1.index, vec![(id, paring_1.pairing.clone())]);
    opener_map.insert(paring_2.index, vec![(id, paring_2.pairing.clone())]);

    let opened_ids = open(&y, &grpkey, &gml.keys().copied().collect(), &opener_map);

    assert_eq!(partial_memkey.sigma1, memkey1.sigma1);
    assert_eq!(partial_memkey.sigma2, memkey1.sigma2);
    assert_eq!(partial_memkey.e1, memkey1.e1);
    assert_eq!(sk, memkey1.sk);

    assert!(verify(&y, &grpkey, "Hallo".as_bytes()).is_ok());
    assert_eq!(opened_ids[0], id);
}

#[test]
fn test_shares_verifiability() {
    let (grpkey, _) = setup();
    let id = 0usize;
    let sk = FieldElement::random();
    let h = h0(&id);
    let (shares, commits) = shamir_feldman_share(&sk, 3, 6, &(&grpkey.g * &h));

    let z = recover_secret(&shares[..3]);
    assert_eq!(sk, z);

    for (i, share) in shares.iter().enumerate() {
        let is_valid = verify_share(&(&grpkey.g * &h), share.clone(), &commits);
        assert!(is_valid, "Share {i} failed verification");
    }
}

#[test]
fn test_elgamal_encryption() {
    let (grpkey, _) = setup();
    let (sk, pk) = elgamal_keygen(&grpkey.g_tilde);

    let m = FieldElement::random();

    let r = FieldElement::random();
    let c0 = &grpkey.g_tilde * &r;
    let c1 = (pk * &r) + (&grpkey.y1_pub_tilde * &m);

    let decrypt = c1 - (c0 * sk);

    assert_eq!(decrypt, &grpkey.y1_pub_tilde * m);
}

#[test]
fn test_correct_encryption_proof() {
    let (grpkey, _) = setup();
    let id = 0usize;
    let sk = FieldElement::random();
    let h = h0(&id);

    // Feldman commitments: create t-out-of-n shares
    let t = 3;
    let n = 5;
    let (shares, commitments) = shamir_feldman_share(&sk, t, n, &(&grpkey.g * &h));

    // Pick a share and simulate an opener’s key
    let share = &shares[0];
    let (_, pk) = elgamal_keygen(&grpkey.g_tilde);

    // Create the NIZK proof
    let enc_share = prove_enc_share(&grpkey, share, &pk, &(&grpkey.g * &h), &commitments);

    // Verify the proof
    let is_valid = verify_enc_share_correctness(&enc_share, &grpkey, &pk, &commitments);

    assert!(is_valid, "Encryption proof failed to verify");
}

#[test]
fn test_signature_bincode_roundtrip() {
    // The compact encoding on Signature is a wire-format change, so a value
    // must survive a bincode round trip byte-for-byte identical.
    let sig = Signature {
        sigma1: G1::random(),
        sigma2: G1::random(),
        c: FieldElement::random(),
        s: FieldElement::random(),
    };

    let bytes = bincode::serialize(&sig).expect("serialize");
    let back: Signature = bincode::deserialize(&bytes).expect("deserialize");

    assert_eq!(sig.sigma1, back.sigma1, "sigma1 did not survive the round trip");
    assert_eq!(sig.sigma2, back.sigma2, "sigma2 did not survive the round trip");
    assert_eq!(sig.c, back.c, "c did not survive the round trip");
    assert_eq!(sig.s, back.s, "s did not survive the round trip");
    assert_eq!(sig, back);
}

/// With threshold t, partial openings from t - 1 openers must not identify the signer.
#[test]
fn test_opening_needs_threshold_openers() {
    let mut gml: HashMap<usize, String> = HashMap::new();
    let (grpkey, mgrkey) = setup();
    let (opener_sec_keys, opener_pub_keys): (Vec<_>, Vec<_>) =
        (0..4).map(|_| elgamal_keygen(&grpkey.g_tilde)).unzip();
    let threshold = 3;
    let n = join_mgr1();
    let id = 7usize;
    let (g_sk, h_sk, pi1, pi2, pi3, sk, enc_shares, commits) =
        join_mem1(id, n.clone(), &grpkey, threshold, &opener_pub_keys);
    let partial_memkey = join_mgr2(
        id,
        JoinMgr2Args {
            gml: &mut gml,
            name: "member".to_string(),
            mgrkey: &mgrkey,
            grpkey: &grpkey,
            n: &n,
            g_sk: &g_sk,
            h_sk: &h_sk,
            opener_pubkeys: &opener_pub_keys,
            commits: &commits,
            pi1,
            pi2,
            pi3,
            enc_shares: &enc_shares,
        },
    )
    .unwrap();
    let memkey = join_mem2(&sk, &partial_memkey);
    let sig = sign(&memkey, &grpkey, &"msg".as_bytes()).unwrap();
    let members: HashSet<usize> = gml.keys().copied().collect();

    let partial = |openers: usize| {
        let mut all_sj = HashMap::new();
        for j in 0..openers {
            let p = create_open_pairing(&sig, enc_shares[j].clone(), &opener_sec_keys[j]);
            all_sj.insert(p.index, vec![(id, p.pairing)]);
        }
        all_sj
    };

    assert!(open(&sig, &grpkey, &members, &partial(threshold - 1)).is_empty());
    assert_eq!(open(&sig, &grpkey, &members, &partial(threshold)), vec![id]);
    assert_eq!(open(&sig, &grpkey, &members, &partial(4)), vec![id]);
}

/// The signature a non-member gets after the network decodes sigma1 and sigma2 from bytes that
/// are not curve points: the identity, with c hashing verify()'s own transcript.
fn forged_identity_signature(grpkey: &Grpkey, msg: &[u8]) -> Signature {
    let sigma1 = G1::identity();
    let sigma2 = G1::identity();
    let s = FieldElement::random();
    let c = FieldElement::random();
    // r_pub as verify() computes it; every pairing with the identity is 1
    let r_pub = (GT::ate_pairing(&(&sigma1).neg(), &grpkey.x_pub_tilde)
        * GT::ate_pairing(&sigma2, &grpkey.g_tilde)).pow(&c).inverse()
        * GT::ate_pairing(&(&sigma1 * &s), &grpkey.y1_pub_tilde);
    let mut transcript: Vec<u8> = Vec::new();
    transcript.extend(grpkey.to_bytes());
    transcript.extend(sigma1.to_bytes(false));
    transcript.extend(sigma2.to_bytes(false));
    transcript.extend(r_pub.to_bytes());
    transcript.extend(bincode::serialize(&msg).unwrap());
    Signature { sigma1, sigma2, c: FieldElement::from_msg_hash(&transcript), s }
}

#[test]
fn test_identity_signature_rejected() {
    let (grpkey, _) = setup();
    let msg = "forged".as_bytes();
    let forged = forged_identity_signature(&grpkey, msg);
    assert!(verify(&forged, &grpkey, &msg).is_err(), "a non-member forged a valid group signature");
    // Opening it must not name every member.
    let members: HashSet<usize> = [1usize, 2, 3].into_iter().collect();
    let all_sj: HashMap<usize, Vec<(usize, GT)>> =
        (1..=3).map(|j| (j, members.iter().map(|m| (*m, GT::one())).collect())).collect();
    assert!(open(&forged, &grpkey, &members, &all_sj).is_empty());
}

#[test]
fn test_off_curve_signature_bytes_rejected() {
    // [u64 len][sigma1][u64 len][sigma2][u64 len][c][u64 len][s]
    let honest = Signature { sigma1: G1::random(), sigma2: G1::random(), c: FieldElement::one(), s: FieldElement::one() };
    let mut bytes = bincode::serialize(&honest).unwrap();
    assert!(bincode::deserialize::<Signature>(&bytes).is_ok());
    let len = u64::from_le_bytes(bytes[0..8].try_into().unwrap()) as usize;
    let off_curve = (1u8..=255)
        .map(|x| { let mut b = vec![0u8; len]; b[0] = bytes[8]; b[len - 1] = x; b })
        .find(|b| G1::from(amcl_wrapper::types::GroupG1::frombytes(b)).is_identity())
        .expect("no off-curve encoding found");
    bytes[8..8 + len].copy_from_slice(&off_curve);
    assert!(bincode::deserialize::<Signature>(&bytes).is_err());
}

/// An authority with `n` openers and a fresh nonce, as join_mgr2 sees it.
fn join_setup(n: usize) -> (Grpkey, Mgrkey, Vec<G2>, Vec<FieldElement>, G1) {
    let (grpkey, mgrkey) = setup();
    let (sks, pks): (Vec<_>, Vec<_>) = (0..n).map(|_| elgamal_keygen(&grpkey.g_tilde)).unzip();
    (grpkey, mgrkey, pks, sks, join_mgr1())
}

fn try_join(
    id: usize,
    grpkey: &Grpkey,
    mgrkey: &Mgrkey,
    opener_pub_keys: &[G2],
    n: &G1,
    material: &JoinMem1Output,
    enc_shares: &[EncShare],
    commits: &[G1],
) -> Result<Memkey, String> {
    let (g_sk, h_sk, pi1, pi2, pi3, _, _, _) = material;
    let mut gml = HashMap::new();
    join_mgr2(
        id,
        JoinMgr2Args {
            gml: &mut gml,
            name: "member".to_string(),
            mgrkey,
            grpkey,
            n,
            g_sk,
            h_sk,
            opener_pubkeys: opener_pub_keys,
            commits,
            pi1: pi1.clone(),
            pi2: pi2.clone(),
            pi3: pi3.clone(),
            enc_shares,
        },
    )
}

#[test]
fn test_join_rejects_shares_of_another_secret() {
    let (grpkey, mgrkey, pks, _, n) = join_setup(4);
    let id = 3usize;
    let material = join_mem1(id, n.clone(), &grpkey, 3, &pks);
    let (_, _, _, _, _, _, enc_shares, commits) = &material;
    assert!(try_join(id, &grpkey, &mgrkey, &pks, &n, &material, enc_shares, commits).is_ok());

    // Valid shares and share proofs, but of a secret the member does not sign with:
    // the archives could never open this member.
    let (other_shares, other_commits) =
        create_enc_shares(&id, &grpkey, 3, &pks, &FieldElement::random());
    assert!(try_join(id, &grpkey, &mgrkey, &pks, &n, &material, &other_shares, &other_commits).is_err());
}

#[test]
fn test_join_rejects_shares_that_cannot_be_interpolated() {
    let (grpkey, mgrkey, pks, _, n) = join_setup(4);
    let id = 5usize;
    let material = join_mem1(id, n.clone(), &grpkey, 3, &pks);
    let (_, _, _, _, _, sk, enc_shares, commits) = &material;

    // Every archive gets a correctly proven share at index 1 of a sharing of sk, so no t
    // distinct indices exist and no opening can ever interpolate sk.
    let h_blind = enc_shares[0].h_blind.clone();
    let (s, c) = shamir_feldman_share(sk, 3, 4, &h_blind);
    let same_index: Vec<EncShare> =
        pks.iter().map(|pk| prove_enc_share(&grpkey, &s[0], pk, &h_blind, &c)).collect();
    assert!(same_index.iter().zip(&pks).all(|(e, pk)| verify_enc_share_correctness(e, &grpkey, pk, &c)));
    assert!(try_join(id, &grpkey, &mgrkey, &pks, &n, &material, &same_index, &c).is_err());

    // Too few shares for the archives.
    assert!(try_join(id, &grpkey, &mgrkey, &pks, &n, &material, &enc_shares[..3], commits).is_err());
}

#[test]
fn test_member_proof_for_additional_peer() {
    let (grpkey, mgrkey, pks, opener_sks, n) = join_setup(4);
    let id = 9usize;
    let material = join_mem1(id, n.clone(), &grpkey, 3, &pks);
    let (_, _, _, _, _, sk, enc_shares, commits) = &material;
    let memkey = join_mem2(sk, &try_join(id, &grpkey, &mgrkey, &pks, &n, &material, enc_shares, commits).unwrap());

    // The member shares its key again, under a fresh base, for a new sharing peer, and uses
    // its slot 1 of epoch 5.
    let (peer_shares, peer_commits) = create_enc_shares(&id, &grpkey, 3, &pks, &memkey.sk);
    let h_blind = check_share_layout(&peer_shares, &peer_commits, pks.len()).unwrap();
    let slot = tags::tag(&memkey.sk, 5, 1);
    let pairs = [(h_blind.clone(), peer_commits[0].clone()), tags::statement(&slot)];
    let context = b"keypackage of the new peer";
    let proof = prove_member(&memkey, &grpkey, &pairs, context).unwrap();
    let proof: MemberProof = bincode::deserialize(&bincode::serialize(&proof).unwrap()).unwrap();
    assert!(verify_member(&proof, &grpkey, &pairs, context));
    assert!(!verify_member(&proof, &grpkey, &pairs, b"another peer"));

    // Shares of another secret, or a tag of another key, cannot be proven.
    let (bad_shares, bad_commits) = create_enc_shares(&id, &grpkey, 3, &pks, &FieldElement::random());
    let bad_h = check_share_layout(&bad_shares, &bad_commits, pks.len()).unwrap();
    let bad_pairs = [(bad_h, bad_commits[0].clone()), tags::statement(&slot)];
    let bad_proof = prove_member(&memkey, &grpkey, &bad_pairs, context).unwrap();
    assert!(!verify_member(&bad_proof, &grpkey, &bad_pairs, context));
    let foreign = tags::tag(&FieldElement::random(), 5, 1);
    let foreign_pairs = [pairs[0].clone(), tags::statement(&foreign)];
    let foreign_proof = prove_member(&memkey, &grpkey, &foreign_pairs, context).unwrap();
    assert!(!verify_member(&foreign_proof, &grpkey, &foreign_pairs, context));

    // A relabeled tag (other epoch or slot, same value) fails too.
    let relabeled = SlotTag { slot: 2, ..slot.clone() };
    let relabeled_pairs = [pairs[0].clone(), tags::statement(&relabeled)];
    assert!(!verify_member(&proof, &grpkey, &relabeled_pairs, context));

    // The linked peer shares open the member's signatures.
    let sig = sign(&memkey, &grpkey, &"msg".as_bytes()).unwrap();
    let peer = "new-peer".to_string();
    let mut all_sj = HashMap::new();
    for j in 0..3 {
        let p = create_open_pairing(&sig, peer_shares[j].clone(), &opener_sks[j]);
        all_sj.insert(p.index, vec![(peer.clone(), p.pairing)]);
    }
    let peers: HashSet<String> = [peer.clone()].into_iter().collect();
    assert_eq!(open(&sig, &grpkey, &peers, &all_sj), vec![peer]);
}

#[test]
fn test_slot_tags() {
    let (sk, other) = (FieldElement::random(), FieldElement::random());
    // Deterministic per member, epoch and slot, so the AA can refuse a second use.
    assert_eq!(tags::tag(&sk, 5, 1), tags::tag(&sk, 5, 1));
    // Different for another slot, epoch or member.
    assert_ne!(tags::tag(&sk, 5, 1).tag, tags::tag(&sk, 5, 2).tag);
    assert_ne!(tags::tag(&sk, 5, 1).tag, tags::tag(&sk, 6, 1).tag);
    assert_ne!(tags::tag(&sk, 5, 1).tag, tags::tag(&other, 5, 1).tag);
    let tag = tags::tag(&sk, 5, 1);
    let received: SlotTag = bincode::deserialize(&bincode::serialize(&tag).unwrap()).unwrap();
    assert_eq!(received, tag);
}
