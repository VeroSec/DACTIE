//! Slot tags cap a member's sharing peers per epoch without revealing the member. A member has
//! slots 1..=k per epoch, and its tag for a slot is H(epoch, slot)^sk: the same member always
//! gets the same tag for the same epoch and slot, so the AA accepts each tag once and at most k
//! per member and epoch, while tags of different members, slots or epochs cannot be linked
//! (XDH in G1). A peer proves its tag with prove_member, together with any other statements
//! about sk. This is k-times anonymous authentication with per-epoch tags.

use crate::structs::SlotTag;
use amcl_wrapper::field_elem::FieldElement;
use amcl_wrapper::group_elem::GroupElement;
use amcl_wrapper::group_elem_g1::G1;

/// H(epoch, slot), the base of every member's tag for this slot.
pub fn tag_base(epoch: u64, slot: u32) -> G1 {
    let mut input = b"dactie-slot-tag|".to_vec();
    input.extend(epoch.to_be_bytes());
    input.extend(slot.to_be_bytes());
    G1::from_msg_hash(&input)
}

pub fn tag(sk: &FieldElement, epoch: u64, slot: u32) -> SlotTag {
    SlotTag {
        epoch,
        slot,
        tag: tag_base(epoch, slot) * sk,
    }
}

/// The (base, value) pair that prove_member and verify_member take for this tag.
pub fn statement(tag: &SlotTag) -> (G1, G1) {
    (tag_base(tag.epoch, tag.slot), tag.tag.clone())
}
