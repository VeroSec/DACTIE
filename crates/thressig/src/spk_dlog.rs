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

pub fn verify(tau: &G1, g: &G1, pi: (FieldElement, FieldElement), msg: &[u8]) -> bool {
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

/// Proves log_g(tau) = log_h(eta) = x, i.e. that tau and eta hide the same x (Chaum-Pedersen).
pub fn sign_eq(
    tau: &G1,
    g: &G1,
    eta: &G1,
    h: &G1,
    msg: &[u8],
    x: &FieldElement,
) -> (FieldElement, FieldElement) {
    let r = FieldElement::random();
    let gr = g * &r;
    let hr = h * &r;
    let c = FieldElement::from_msg_hash(&eq_transcript(tau, g, eta, h, &gr, &hr, msg));
    let s = r - (&c * x);

    (s, c)
}

pub fn verify_eq(
    tau: &G1,
    g: &G1,
    eta: &G1,
    h: &G1,
    pi: (FieldElement, FieldElement),
    msg: &[u8],
) -> bool {
    /* g^s * tau^c = g^r and h^s * eta^c = h^r */
    let gr = &(g * &pi.0) + &(tau * &pi.1);
    let hr = &(h * &pi.0) + &(eta * &pi.1);

    FieldElement::from_msg_hash(&eq_transcript(tau, g, eta, h, &gr, &hr, msg)) == pi.1
}

fn eq_transcript(tau: &G1, g: &G1, eta: &G1, h: &G1, gr: &G1, hr: &G1, msg: &[u8]) -> Vec<u8> {
    let mut msg_to_send: Vec<u8> = Vec::new();
    msg_to_send.extend_from_slice(b"spk-dlog-eq|");
    msg_to_send.extend_from_slice(msg);
    for point in [tau, g, eta, h, gr, hr] {
        msg_to_send.extend(point.to_bytes(false));
    }
    msg_to_send
}
