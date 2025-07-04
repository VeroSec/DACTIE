use super::*;

#[cfg(test)]


#[test]
fn test_setup() {
    let (grpkey, mgrkey) = setup();
    let (grpkey2, mgrkey2) = setup();
    assert_eq!(&(&mgrkey.x*&grpkey.g_tilde), &grpkey.x_pub_tilde);
    assert_eq!(&(&mgrkey.y*&grpkey.g_tilde), &grpkey.y_pub_tilde);
    assert_ne!(&grpkey.g,&grpkey2.g);
    assert_ne!(&grpkey.g_tilde,&grpkey2.g_tilde);
    assert_ne!(mgrkey.x,mgrkey2.x);
    assert_ne!(mgrkey.y,mgrkey2.y);
}

#[test]
fn test_flow() {
    let mut gml:Vec<(usize,G1,G2)> = vec![];

    let (grpkey, mgrkey) = setup();
    let n = join_mgr1();
    let (tau,ttau,pi,sk) = join_mem1(&n, &grpkey);
    let (id,partial_memkey) = join_mgr2(&mut gml, &mgrkey, &grpkey, &n, &tau, &ttau, pi).unwrap();
    let memkey1= join_mem2(&sk, &partial_memkey);
    let y = sign(&memkey1, "Hallo".as_bytes());

    
    assert_eq!(partial_memkey.sigma1,memkey1.sigma1);
    assert_eq!(partial_memkey.sigma2,memkey1.sigma2);
    assert_eq!(partial_memkey.e,memkey1.e);
    assert_eq!(sk,memkey1.sk);

    assert_eq!(verify(&y, &grpkey, "Hallo".as_bytes()),true);

    let res = open(&y, &grpkey, &gml).unwrap();
    assert_eq!(res.0,0);
    assert_eq!(open_verify(&y,&grpkey,res.1),true);
}



#[test]
fn test_signature_length() {
    let x =dummy_return_signing_key();
    let sig = dummy_sign(&x,"Hellosadadjasd".as_bytes());
    assert_eq!(sig.to_bytes().len(),SIGNATURE_LENGTH);

}