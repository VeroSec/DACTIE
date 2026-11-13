pub mod admission;

/// The event of a libp2p behaviour that reports nothing to the swarm (`ToSwarm`). An empty
/// enum of our own instead of `Infallible`: since Rust 1.100, `Infallible` is the never type `!`,
/// and the code that `#[derive(NetworkBehaviour)]` generates for it then warns as unreachable.
#[derive(Debug)]
pub enum NoEvent {}
pub mod conn_cap;
pub mod dedup;
pub mod key_store;
pub mod mls_wrapper;
pub mod net;
pub mod persist;
pub mod shared_structs;
pub mod slot_store;
pub mod stem;
