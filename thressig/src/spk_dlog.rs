use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem::GroupElement;
use amcl_wrapper::group_elem_g1::G1;


pub fn sign(tau: &G1, g: &G1, msg: &[u8], x: &FieldElement) -> (FieldElement, FieldElement) {
    let r = FieldElement::random();
    let gr = g * &r;

    /* Make Hash(msg||G||g||g^r) */
    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend_from_slice(msg);
    msg_to_send.extend(tau.to_bytes(false));
    msg_to_send.extend(g.to_bytes(false));
    msg_to_send.extend(gr.to_bytes(false));

    let c = FieldElement::from_msg_hash(&msg_to_send);
    let s = r - (&c * x);


    (s, c)
}

pub fn verify(tau: &G1, g: &G1, pi:(FieldElement,FieldElement),msg: &[u8]) -> bool{
    /* Compute g^pi->s * g^pi->c */
    let gs = g * &pi.0;
    let tauc = tau * &pi.1;
    let gstauc = &gs + &tauc;

    /* Make Hash(msg||G||g||g^pi->s*g^pi->c)) */
    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend_from_slice(msg);
    msg_to_send.extend(tau.to_bytes(false));
    msg_to_send.extend(g.to_bytes(false));
    msg_to_send.extend(gstauc.to_bytes(false));

    let c = FieldElement::from_msg_hash(&msg_to_send);

    c == pi.1
}