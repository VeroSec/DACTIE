//! Requests this peer is waiting for an answer to.
//!
//! libp2p hands a response only to the request it was sent for and only from the peer the
//! request went to, but that peer can answer with any response: asked for a KeyPackage, it
//! could send a MemKey and replace this node's member key. A response is therefore used only
//! if it is one the request asked for.
//!
//! Lives outside MyBehaviour for the same reason as `archive_query_timing`.

use dactie_utils::shared_structs::{RequestMessage, ResponseMessage};
use libp2p::request_response::OutboundRequestId;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// What a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    KeyPackage,
    JoinGroup,
    Update,
    Identity,
    MemKey,
    Registration,
    Renewal,
    OpenerKeys,
    // Requests a peer does not send; no response answers them.
    Other,
}

impl Asked {
    pub(crate) fn of(request: &RequestMessage) -> Asked {
        match request {
            RequestMessage::KeyPackage => Asked::KeyPackage,
            RequestMessage::JoinGroup { .. } => Asked::JoinGroup,
            RequestMessage::Update { .. } => Asked::Update,
            RequestMessage::VerifyIdentity { .. } => Asked::Identity,
            RequestMessage::JoinKeyMaterial { .. } => Asked::MemKey,
            RequestMessage::AddPeerToSig { .. } => Asked::Registration,
            RequestMessage::RenewPeer { .. } => Asked::Renewal,
            RequestMessage::RequestOpenerKeys => Asked::OpenerKeys,
            _ => Asked::Other,
        }
    }

    /// True if `response` answers this kind of request. The AA answers RequestOpenerKeys with
    /// OpenerKeys, an archive with ArchiveOpenerKey.
    pub(crate) fn answered_by(self, response: &ResponseMessage) -> bool {
        use ResponseMessage as R;
        matches!(
            (self, response),
            (Asked::KeyPackage, R::KeyPackage(_))
                | (Asked::JoinGroup, R::JoinGroup)
                | (Asked::Update, R::ResUpdate { .. })
                | (Asked::Identity, R::IdentityVerified { .. })
                | (Asked::MemKey, R::MemKey { .. })
                | (Asked::Registration, R::PeerAdded | R::PeerRefused { .. })
                | (
                    Asked::Renewal,
                    R::PeerRenewed { .. } | R::RenewRefused { .. }
                )
                | (
                    Asked::OpenerKeys,
                    R::OpenerKeys { .. } | R::ArchiveOpenerKey { .. }
                )
        )
    }
}

fn table() -> &'static Mutex<HashMap<OutboundRequestId, Asked>> {
    static T: OnceLock<Mutex<HashMap<OutboundRequestId, Asked>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn sent(id: OutboundRequestId, asked: Asked) {
    table().lock().unwrap().insert(id, asked);
}

/// Consumes the entry of an answered or failed request: a request is answered once.
pub(crate) fn take(id: &OutboundRequestId) -> Option<Asked> {
    table().lock().unwrap().remove(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer asked for a KeyPackage cannot answer with a member key.
    #[test]
    fn a_response_must_answer_its_request() {
        let memkey = ResponseMessage::MemKey {
            partial_memkey: Some(vec![1]),
        };
        assert!(!Asked::of(&RequestMessage::KeyPackage).answered_by(&memkey));
        assert!(!Asked::of(&RequestMessage::RequestOpenerKeys).answered_by(&memkey));
        assert!(Asked::MemKey.answered_by(&memkey));
        assert!(Asked::KeyPackage.answered_by(&ResponseMessage::KeyPackage(vec![1])));
        assert!(
            Asked::Registration.answered_by(&ResponseMessage::PeerRefused {
                reason: String::new()
            })
        );
        assert!(!Asked::Other.answered_by(&ResponseMessage::JoinGroup));
    }
}
