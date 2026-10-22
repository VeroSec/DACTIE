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
static PENDING: OnceLock<Mutex<HashMap<OutboundRequestId, Pending>>> = OnceLock::new();
static CAPABLE: OnceLock<Mutex<HashSet<PeerId>>> = OnceLock::new();

/// Sets the stem configuration; called once when the behaviour is built.
pub(crate) fn init(config: StemConfig) {
    let _ = STATE.set(Mutex::new(StemState::new(config)));
}

/// Routing state. Do not hold the guard across a call that locks it again.
pub(crate) fn state() -> MutexGuard<'static, StemState> {
    STATE
        .get_or_init(|| Mutex::new(StemState::new(StemConfig { q: 0.0, ..StemConfig::default() })))
        .lock()
        .unwrap()
}

/// A stem message in flight and its stem predecessor (`None` for this peer's own).
pub(crate) type Pending = (StemMessage, Option<PeerId>);

/// Stem requests in flight, published or re-routed if the hop fails.
pub(crate) fn pending() -> MutexGuard<'static, HashMap<OutboundRequestId, Pending>> {
    PENDING.get_or_init(Default::default).lock().unwrap()
}

/// Peers whose identify info lists the stem protocol: sharing peers, never the
/// AA or the archives.
pub(crate) fn capable() -> MutexGuard<'static, HashSet<PeerId>> {
    CAPABLE.get_or_init(Default::default).lock().unwrap()
}
