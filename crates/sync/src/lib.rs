//! Device sync over an opaque byte transport. `remember-core`'s Loro document
//! already merges commutatively and idempotently (see its property tests) —
//! this crate's only job is getting bytes to and from a server and knowing
//! when to call `App`'s sync primitives, not any merge logic of its own.

mod auth;
mod coordinator;
mod engine;
mod transport;

pub use auth::{AuthClient, Session};
pub use coordinator::{sign_out, AutoSyncCoordinator, CoordinatorConfig};
pub use engine::{SyncEngine, SyncOutcome};
pub use transport::{HttpTransport, PullPage, PulledSnapshot, PulledUpdate, SyncTransport};

#[cfg(any(test, feature = "testing"))]
pub use transport::InMemoryTransport;

/// The one HTTP client both [`AuthClient`] and [`HttpTransport`] use. Tests
/// talk to wiremock over plain http, so only they drop `https_only`.
fn http_client() -> reqwest::blocking::Client {
    build_http_client(cfg!(not(test)))
}

/// Never follows redirects: on a 307/308 reqwest would re-send the body
/// (email and password, or a refresh token) to wherever the redirect points.
/// A 3xx comes back as an ordinary non-success response instead.
fn build_http_client(https_only: bool) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .https_only(https_only)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        // Same failure `Client::new()` panics on: the TLS backend can't start.
        .expect("HTTP client should build")
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("core error: {0}")]
    Core(#[from] remember_core::CoreError),
}
