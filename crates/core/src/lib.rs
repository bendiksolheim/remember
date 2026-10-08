#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(feature = "native")]
mod app;
mod civil;
pub mod clock;
pub mod command;
mod doc;
mod due_parse;
mod session;
pub mod snapshot;
#[cfg(feature = "native")]
mod store;

#[cfg(feature = "native")]
pub use app::App;
pub use clock::{fresh_peer_id, Clock, IdSource, SystemClock, UuidSource};
#[cfg(any(test, feature = "testing"))]
pub use clock::{FixedClock, SeqIdSource};
pub use command::{Command, ViewFilter};
pub use doc::Doc;
pub use session::Session;
pub use snapshot::{DueDetection, DueState, ListColor, ListRow, Snapshot, TaskRow};
#[cfg(feature = "native")]
pub use store::Store;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CoreError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("document error: {0}")]
    Document(String),
    #[error("no such task: {0}")]
    NotFound(String),
}
