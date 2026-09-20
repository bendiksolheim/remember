//! Device sync over an opaque byte transport. `todo-core`'s Loro document
//! already merges commutatively and idempotently (see its property tests) —
//! this crate's only job is getting bytes to and from a server and knowing
//! when to call `App`'s sync primitives, not any merge logic of its own.

mod auth;
mod coordinator;
mod engine;
mod transport;

pub use auth::{AuthClient, Session};
pub use coordinator::{AutoSyncCoordinator, CoordinatorConfig};
pub use engine::{SyncEngine, SyncOutcome};
pub use transport::{HttpTransport, PulledUpdate, SyncTransport};

#[cfg(any(test, feature = "testing"))]
pub use transport::InMemoryTransport;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("core error: {0}")]
    Core(#[from] todo_core::CoreError),
}
