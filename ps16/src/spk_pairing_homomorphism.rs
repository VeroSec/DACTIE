use std::ops::Mul;
use amcl_wrapper::extension_field_gt::GT;
use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem::GroupElement;
use amcl_wrapper::group_elem_g2::G2;
use amcl_wrapper::group_elem_g1::G1;

pub fn sign(g: &G1, g_pub: &GT,xx: &G2,msg: &Vec<u8>) -> (FieldElement, G2){
    let rr=G2::random();
    let r_pub = GT::ate_pairing(&g,&rr);


    /* Make c = Hash(msg||g||G||r_pub) */
    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend(msg);
    msg_to_send.extend(g.to_bytes(false));
    msg_to_send.extend(g_pub.to_bytes());
    msg_to_send.extend(r_pub.to_bytes());

    let c = FieldElement::from_msg_hash(&msg_to_send);
    let s = xx * &c + rr;

    (c,s)
}

pub fn verify(g: &G1, g_pub: &GT,pi: &(FieldElement, G2),msg: &[u8]) -> bool{
    /* If pi is correct, then pi->c equals Hash(msg||g||G||e(g,pi->ss)/G^pi->c) */
    let gc = &g_pub.pow(&pi.0);
    let r_pub = GT::ate_pairing(&g,&pi.1).mul(gc.inverse());

    /* Make c = Hash(msg||g||G||e(g,pi->s)/G^pi->c) */
    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend_from_slice(msg);
    msg_to_send.extend(g.to_bytes(false));
    msg_to_send.extend(g_pub.to_bytes());
    msg_to_send.extend(r_pub.to_bytes());

    let c = FieldElement::from_msg_hash(&msg_to_send);

    pi.0 == c

}