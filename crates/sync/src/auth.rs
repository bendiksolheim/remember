//! Thin wrapper over Supabase's GoTrue auth REST API. Rust only ever
//! receives and returns a [`Session`] — persisting it (Keychain) is the
//! caller's job, per the working agreement's "no business logic in Swift"
//! exception for token storage specifically. Refreshing it, unlike storing
//! it, *is* a decision (when, and with which grant), so that stays here:
//! see [`AuthClient::ensure_fresh`].

use std::sync::Arc;

use serde::Deserialize;
use todo_core::Clock;

use crate::SyncError;

/// Refresh this far ahead of the JWT's real expiry, so a sync that starts
/// just under the wire doesn't race the server's own clock and get a 401
/// mid-request instead of a clean proactive refresh.
const REFRESH_LEEWAY_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    /// Unix seconds. Absolute, not a duration, so it survives being stored
    /// and reloaded without needing to know how long ago that happened.
    pub expires_at: i64,
}

impl Session {
    fn needs_refresh(&self, now: i64) -> bool {
        self.expires_at <= now + REFRESH_LEEWAY_SECS
    }
}

#[derive(Clone)]
pub struct AuthClient {
    base_url: String,
    anon_key: String,
    client: reqwest::blocking::Client,
    clock: Arc<dyn Clock>,
}

#[derive(Deserialize)]
struct RawAuthResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    user: RawUser,
}

#[derive(Deserialize)]
struct RawUser {
    id: String,
}

impl AuthClient {
    pub fn new(base_url: impl Into<String>, anon_key: impl Into<String>) -> Self {
        Self::with_clock(base_url, anon_key, Arc::new(todo_core::SystemClock))
    }

    /// Same as [`Self::new`], but with the clock injected — the only way
    /// tests get deterministic control over whether a session "needs
    /// refresh".
    pub fn with_clock(
        base_url: impl Into<String>,
        anon_key: impl Into<String>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            anon_key: anon_key.into(),
            client: reqwest::blocking::Client::new(),
            clock,
        }
    }

    pub fn sign_up(&self, email: &str, password: &str) -> Result<Session, SyncError> {
        self.call(
            "/auth/v1/signup",
            serde_json::json!({ "email": email, "password": password }),
        )
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<Session, SyncError> {
        self.call(
            "/auth/v1/token?grant_type=password",
            serde_json::json!({ "email": email, "password": password }),
        )
    }

    fn refresh(&self, refresh_token: &str) -> Result<Session, SyncError> {
        self.call(
            "/auth/v1/token?grant_type=refresh_token",
            serde_json::json!({ "refresh_token": refresh_token }),
        )
    }

    /// The one place callers should get a "good to use right now" session
    /// from. Refreshes `session` when its access token is at or near expiry
    /// and returns it unchanged otherwise -- so a caller can always just
    /// call this before using a session, rather than separately deciding
    /// whether to.
    pub fn ensure_fresh(&self, session: Session) -> Result<Session, SyncError> {
        if session.needs_refresh(self.clock.now()) {
            self.refresh(&session.refresh_token)
        } else {
            Ok(session)
        }
    }

    fn call(&self, path: &str, body: serde_json::Value) -> Result<Session, SyncError> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(url)
            .header("apikey", &self.anon_key)
            .json(&body)
            .send()
            .map_err(|e| SyncError::Auth(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SyncError::Auth(format!(
                "auth request failed: HTTP {}",
                resp.status()
            )));
        }
        let raw: RawAuthResponse = resp
            .json()
            .map_err(|e| SyncError::Auth(format!("invalid auth response: {e}")))?;
        Ok(Session {
            access_token: raw.access_token,
            refresh_token: raw.refresh_token,
            user_id: raw.user.id,
            expires_at: self.clock.now() + raw.expires_in,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use todo_core::FixedClock;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn session_with_expiry(expires_at: i64) -> Session {
        Session {
            access_token: "old-access".to_string(),
            refresh_token: "old-refresh".to_string(),
            user_id: "user-789".to_string(),
            expires_at,
        }
    }

    fn canned_response() -> serde_json::Value {
        serde_json::json!({
            "access_token": "access-123",
            "refresh_token": "refresh-456",
            "token_type": "bearer",
            "expires_in": 3600,
            "user": { "id": "user-789", "email": "a@example.com" }
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_up_posts_to_signup_endpoint_with_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/signup"))
            .and(header("apikey", "anon-key"))
            .and(body_json(serde_json::json!({
                "email": "a@example.com",
                "password": "hunter2"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = tokio::task::spawn_blocking(move || {
            AuthClient::new(base, "anon-key").sign_up("a@example.com", "hunter2")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(session.access_token, "access-123");
        assert_eq!(session.refresh_token, "refresh-456");
        assert_eq!(session.user_id, "user-789");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_in_posts_to_token_endpoint_with_password_grant() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = tokio::task::spawn_blocking(move || {
            AuthClient::new(base, "anon-key").sign_in("a@example.com", "hunter2")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(session.access_token, "access-123");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_success_status_is_an_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/signup"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid credentials"
            })))
            .mount(&server)
            .await;

        let base = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::new(base, "anon-key").sign_up("a@example.com", "bad")
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(SyncError::Auth(_))));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn malformed_response_body_is_an_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/signup"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let base = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::new(base, "anon-key").sign_up("a@example.com", "x")
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(SyncError::Auth(_))));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_up_computes_expires_at_from_the_clock_and_expires_in() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/signup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(1_000)))
                .sign_up("a@example.com", "hunter2")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(session.expires_at, 1_000 + 3_600);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ensure_fresh_leaves_a_session_alone_well_before_expiry() {
        // No mock mounted for the token endpoint -- if `ensure_fresh` called
        // it anyway, this test fails on the unexpected request instead of
        // silently passing.
        let server = MockServer::start().await;
        let base = server.uri();
        let session = session_with_expiry(10_000);
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0))).ensure_fresh(session)
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.access_token, "old-access");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ensure_fresh_refreshes_a_session_right_at_the_leeway_boundary() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .and(body_json(
                serde_json::json!({ "refresh_token": "old-refresh" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .mount(&server)
            .await;

        let base = server.uri();
        // Exactly `REFRESH_LEEWAY_SECS` out -- `needs_refresh` treats "due
        // within the leeway window" as due now, not just "already expired".
        let session = session_with_expiry(REFRESH_LEEWAY_SECS);
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0))).ensure_fresh(session)
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.access_token, "access-123");
        assert_eq!(result.refresh_token, "refresh-456");
        assert_eq!(result.expires_at, 3_600);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ensure_fresh_refreshes_an_already_expired_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = session_with_expiry(-1);
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0))).ensure_fresh(session)
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.access_token, "access-123");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ensure_fresh_surfaces_a_failed_refresh_as_an_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant"
            })))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = session_with_expiry(-1);
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0))).ensure_fresh(session)
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(SyncError::Auth(_))));
    }
}
