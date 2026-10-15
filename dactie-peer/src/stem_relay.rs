//! State of the stem phase (Dandelion++) for this sharing peer.
//!
//! `MyBehaviour` can only hold behaviours (libp2p's derive), so, like
//! `archive_keys` and `admission`, the stem keeps its state here. The routing
//! itself is `dactie_utils::stem`.

use dactie_utils::stem::{StemConfig, StemMessage, StemState};
use libp2p::request_response::OutboundRequestId;
use libp2p::PeerId;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

static STATE: OnceLock<Mutex<StemState>> = OnceLock::new();
static PENDING: OnceLock<Mutex<HashMap<OutboundRequestId, StemMessage>>> = OnceLock::new();
static CAPABLE: OnceLock<Mutex<HashSet<PeerId>>> = OnceLock::new();
static LOCAL: OnceLock<PeerId> = OnceLock::new();

/// Sets the stem configuration; called once when the behaviour is built.
pub(crate) fn init(config: StemConfig, local: PeerId) {
    let _ = STATE.set(Mutex::new(StemState::new(config)));
    let _ = LOCAL.set(local);
}

/// This peer's PeerID, the propagation source of messages it publishes itself.
pub(crate) fn local_peer() -> PeerId {
    *LOCAL.get().expect("stem_relay::init runs when the behaviour is built")
}

/// Routing state. Do not hold the guard across a call that locks it again.
pub(crate) fn state() -> MutexGuard<'static, StemState> {
    STATE
        .get_or_init(|| Mutex::new(StemState::new(StemConfig { q: 0.0, ..StemConfig::default() })))
        .lock()
        .unwrap()
}

/// Stem requests in flight, published or re-routed if the hop fails.
pub(crate) fn pending() -> MutexGuard<'static, HashMap<OutboundRequestId, StemMessage>> {
    PENDING.get_or_init(Default::default).lock().unwrap()
}

/// Peers whose identify info lists the stem protocol: sharing peers, never the
/// AA or the archives.
pub(crate) fn capable() -> MutexGuard<'static, HashSet<PeerId>> {
    CAPABLE.get_or_init(Default::default).lock().unwrap()
}
