//! Blind registration tokens. Each epoch, the AA signs k blinded tokens per member and a member
//! spends one to register a sharing peer. The AA sees the member when it signs, but not the
//! token, so a spent token does not reveal which member registered the peer. These are blind
//! BLS signatures (Boldyreva, PKC 2003) in the pairing group of the group signature.
//!
//! The AA uses one key per epoch, so tokens cannot be saved for later epochs. Members check
//! every token against the epoch's public key: an AA that signed with a key per member could
//! otherwise recognize the member when the token is spent. All members must see the same
//! public key for this check to hold.

use crate::structs::{BlindedToken, Token};
use amcl_wrapper::extension_field_gt::GT;
use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem::GroupElement;
use amcl_wrapper::group_elem_g1::G1;
use amcl_wrapper::group_elem_g2::G2;

/// The AA's secret token key for `epoch`, derived from its master secret.
pub fn epoch_key(master: &FieldElement, epoch: u64) -> FieldElement {
    let mut input = b"dactie-token-key|".to_vec();
    input.extend(master.to_bytes());
    input.extend(epoch.to_be_bytes());
    FieldElement::from_msg_hash(&input)
}

/// The public key members check tokens of `epoch` against.
pub fn epoch_pubkey(g_tilde: &G2, key: &FieldElement) -> G2 {
    g_tilde * key
}

fn token_point(epoch: u64, serial: &[u8]) -> G1 {
    let mut input = b"dactie-token|".to_vec();
    input.extend(epoch.to_be_bytes());
    input.extend_from_slice(serial);
    G1::from_msg_hash(&input)
}

/// Member: draws a fresh token for `epoch` and blinds it.
pub fn blind(epoch: u64) -> BlindedToken {
    let serial = FieldElement::random().to_bytes();
    let r = FieldElement::random();
    let blinded = token_point(epoch, &serial) * &r;
    BlindedToken {
        epoch,
        serial,
        r,
        blinded,
    }
}

/// AA: signs a blinded token without learning it.
pub fn sign_blinded(key: &FieldElement, blinded: &G1) -> G1 {
    blinded * key
}

/// Member: removes the blinding and checks the AA's signature against the epoch's public key.
pub fn unblind(
    blinded: &BlindedToken,
    signed: &G1,
    g_tilde: &G2,
    epoch_pubkey: &G2,
) -> Option<Token> {
    let token = Token {
        epoch: blinded.epoch,
        serial: blinded.serial.clone(),
        sig: signed * &blinded.r.inverse(),
    };
    verify(&token, g_tilde, epoch_pubkey).then_some(token)
}

/// Checks a token with the public key: e(sig, g~) = e(H(epoch, serial), X~).
pub fn verify(token: &Token, g_tilde: &G2, epoch_pubkey: &G2) -> bool {
    !token.sig.is_identity()
        && GT::ate_pairing(&token.sig, g_tilde)
            == GT::ate_pairing(&token_point(token.epoch, &token.serial), epoch_pubkey)
}

/// AA: checks a token with its secret key, without pairings.
pub fn verify_with_key(token: &Token, key: &FieldElement) -> bool {
    !token.sig.is_identity() && token.sig == token_point(token.epoch, &token.serial) * key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_issue_and_spend() {
        let g_tilde = G2::random();
        let master = FieldElement::random();
        let key = epoch_key(&master, 7);
        let pubkey = epoch_pubkey(&g_tilde, &key);

        let blinded = blind(7);
        // The AA only sees a uniformly random point, not H(epoch, serial).
        assert_ne!(blinded.blinded, token_point(7, &blinded.serial));
        let signed = sign_blinded(&key, &blinded.blinded);
        let token = unblind(&blinded, &signed, &g_tilde, &pubkey).unwrap();

        let token: Token = bincode::deserialize(&bincode::serialize(&token).unwrap()).unwrap();
        assert!(verify(&token, &g_tilde, &pubkey));
        assert!(verify_with_key(&token, &key));
    }

    #[test]
    fn test_token_bound_to_epoch_and_key() {
        let g_tilde = G2::random();
        let master = FieldElement::random();
        let key = epoch_key(&master, 7);
        let blinded = blind(7);
        let token = unblind(
            &blinded,
            &sign_blinded(&key, &blinded.blinded),
            &g_tilde,
            &epoch_pubkey(&g_tilde, &key),
        )
        .unwrap();

        // Not valid in the next epoch, neither relabeled nor under that epoch's key.
        let next = epoch_key(&master, 8);
        assert!(!verify_with_key(&token, &next));
        let relabeled = Token { epoch: 8, ..token.clone() };
        assert!(!verify_with_key(&relabeled, &next));
        assert!(!verify_with_key(&relabeled, &key));

        // A member detects an answer signed with another key, e.g. one kept for tagging it.
        let tagging_key = FieldElement::random();
        assert!(unblind(
            &blinded,
            &sign_blinded(&tagging_key, &blinded.blinded),
            &g_tilde,
            &epoch_pubkey(&g_tilde, &key)
        )
        .is_none());

        // Forged signatures fail.
        let forged = Token { sig: G1::random(), ..token.clone() };
        assert!(!verify_with_key(&forged, &key));
        assert!(!verify(&forged, &g_tilde, &epoch_pubkey(&g_tilde, &key)));
        let identity = Token { sig: G1::identity(), ..token };
        assert!(!verify(&identity, &g_tilde, &epoch_pubkey(&g_tilde, &key)));
    }
}
