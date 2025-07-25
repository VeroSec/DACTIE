use super::*;

#[cfg(test)]


#[test]
fn test_setup() {
    let (grpkey, mgrkey) = setup();
    let (grpkey2, mgrkey2) = setup();
    assert_eq!(&(&mgrkey.x*&grpkey.g_tilde), &grpkey.x_pub_tilde);
    assert_eq!(&(&mgrkey.y1*&grpkey.g_tilde), &grpkey.y1_pub_tilde);
    assert_eq!(&(&mgrkey.y2*&grpkey.g_tilde), &grpkey.y2_pub_tilde);
    assert_ne!(&grpkey.g,&grpkey2.g);
    assert_ne!(&grpkey.g_tilde,&grpkey2.g_tilde);
    assert_ne!(mgrkey.x,mgrkey2.x);
    assert_ne!(mgrkey.y1,mgrkey2.y1);
}

#[test]
fn test_flow() {
    let mut gml:HashMap<usize,String> = HashMap::new();
    let (grpkey, mgrkey) = setup();
    let mut opener_pub_keys= Vec::new();
    let mut opener_sec_keys= Vec::new();
    for _ in 1..=6{
        let (sk,pk) = elgamal_keygen(&grpkey.g_tilde);
        opener_pub_keys.push(pk);
        opener_sec_keys.push(sk);
    }
    let n = join_mgr1();
    let id = 0usize;
    let (g_sk,h_sk,pi1,pi2,sk, enc_shares, commits) = join_mem1(id,n.clone(), &grpkey, 3,&opener_pub_keys);
    let partial_memkey = join_mgr2(id,&mut gml, "test".to_string(),&mgrkey, &grpkey, &n, &g_sk, &h_sk, &opener_pub_keys, &commits,pi1,pi2, &enc_shares).unwrap();
    let memkey1= join_mem2(&sk, &partial_memkey);
    let y = sign(&memkey1,&grpkey, &"Hallo".as_bytes()).unwrap();
    
    
    //Simuluate 3 Openers who work together 
    let paring_0 = &create_open_pairing(&y, enc_shares[0].clone(),&opener_sec_keys[0]);
    let paring_1 = &create_open_pairing(&y, enc_shares[1].clone(),&opener_sec_keys[1]);
    let paring_2 = &create_open_pairing(&y, enc_shares[2].clone(),&opener_sec_keys[2]);
    
    let mut opener_map = HashMap::new();
    opener_map.insert(paring_0.index, vec![(id,paring_0.pairing.clone())]);
    opener_map.insert(paring_1.index, vec![(id,paring_1.pairing.clone())]);
    opener_map.insert(paring_2.index, vec![(id,paring_2.pairing.clone())]);
    
    
    let opened_ids = open(&y,&grpkey,&gml.keys().copied().collect(),&opener_map);

    assert_eq!(partial_memkey.sigma1,memkey1.sigma1);
    assert_eq!(partial_memkey.sigma2,memkey1.sigma2);
    assert_eq!(partial_memkey.e1,memkey1.e1);
    assert_eq!(sk,memkey1.sk);
    

    assert!(verify(&y, &grpkey, "Hallo".as_bytes()).is_ok());
    assert_eq!(opened_ids[0],id);
}


#[test]
fn test_shares_verifiability() {
    let (grpkey, _) = setup();
    let id = 0usize;
    let sk = FieldElement::random();
    let h = h0(&id);
    let (shares,commits)= shamir_feldman_share(&sk,3,6,&(&grpkey.g*&h));
    
    let z = recover_secret(&shares[..3]);
    assert_eq!(sk, z);

    for (i, share) in shares.iter().enumerate() {
        let is_valid = verify_share(&(&grpkey.g * &h), share.clone(), &commits);
        assert!(is_valid, "Share {} failed verification", i);
    }

}

#[test]
fn test_elgamal_encryption() {
    let (grpkey, _) = setup();
    let (sk,pk) = elgamal_keygen(&grpkey.g_tilde);
    
    let m= FieldElement::random();
    
    let r = FieldElement::random();
    let c0 = &grpkey.g_tilde * &r;
    let c1 = (pk * &r) + (&grpkey.y1_pub_tilde * &m);
    
    let decrypt = c1-(c0*sk);
    
    assert_eq!(decrypt, &grpkey.y1_pub_tilde*m);
    
}


#[test]
fn test_correct_encryption_proof() {
    let (grpkey, _) = setup();
    let id = 0usize;
    let sk = FieldElement::random();
    let h= h0(&id);
    
    // Feldman commitments: create t-out-of-n shares
    let t = 3;
    let n = 5;
    let (shares, commitments) = shamir_feldman_share(&sk, t, n, &(&grpkey.g*&h));


    // Pick a share and simulate an opener’s key
    let share = &shares[0];
    let (_,pk) = elgamal_keygen(&grpkey.g_tilde);

    // Create the NIZK proof
    let enc_share = prove_enc_share(
        &grpkey, share,&pk,&(&grpkey.g * &h),  &commitments
    );

    // Verify the proof
    let is_valid = verify_enc_share_correctness(
        &enc_share, &grpkey, &pk,  &commitments
    );

    assert!(is_valid, "Encryption proof failed to verify");
}

