//! Thin wrapper over Supabase's GoTrue auth REST API. Rust only ever
//! receives and returns a [`Session`] — persisting it (Keychain) is the
//! caller's job, per the working agreement's "no business logic in Swift"
//! exception for token storage specifically. Refreshing it, unlike storing
//! it, *is* a decision (when, and with which grant), so that stays here:
//! see [`AuthClient::ensure_fresh`].

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use serde::Deserialize;
use todo_core::Clock;

use crate::SyncError;

/// Refresh this far ahead of the JWT's real expiry, so a sync that starts
/// just under the wire doesn't race the server's own clock and get a 401
/// mid-request instead of a clean proactive refresh.
const REFRESH_LEEWAY_SECS: i64 = 60;

const REFRESH_PATH: &str = "/auth/v1/token?grant_type=refresh_token";

/// `scope=local` revokes only the session the access token belongs to:
/// this device's. Supabase's default is `global`, which revokes every
/// session the user has and so would sign out all their other devices too.
const LOGOUT_PATH: &str = "/auth/v1/logout?scope=local";

/// The statuses GoTrue uses for an access token whose session no longer exists
/// (revoked, expired, or never existed) -- as far as signing out is
/// concerned, that's the same as having revoked it.
fn is_session_gone(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 404)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    /// The account's email, for showing who is signed in. `None` for a
    /// response without one (Supabase also supports phone sign-in).
    pub email: Option<String>,
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
    #[serde(default)]
    email: Option<String>,
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
            client: crate::http_client(),
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
            REFRESH_PATH,
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
            let fresh = self.refresh(&session.refresh_token)?;
            // Only for display, so a refresh response that happens to leave
            // it out shouldn't make the UI forget who is signed in.
            Ok(Session {
                email: fresh.email.or(session.email),
                ..fresh
            })
        } else {
            Ok(session)
        }
    }

    /// Revokes `session` server-side, and only it: the user's sessions on
    /// other devices stay valid (see [`LOGOUT_PATH`]). The logout endpoint
    /// needs a valid access token, so an expired one is refreshed first.
    ///
    /// `Ok` also when the session turns out to be already gone (its refresh
    /// or access token is rejected) -- there's nothing left to revoke. `Err`
    /// means it may still be valid, e.g. the server was unreachable.
    pub fn sign_out(&self, session: Session) -> Result<(), SyncError> {
        let session = if session.needs_refresh(self.clock.now()) {
            let resp = self.post(
                REFRESH_PATH,
                None,
                Some(serde_json::json!({ "refresh_token": session.refresh_token })),
            )?;
            // GoTrue answers a revoked or unknown refresh token with a 400.
            if resp.status() == reqwest::StatusCode::BAD_REQUEST || is_session_gone(resp.status()) {
                return Ok(());
            }
            self.session_from(resp)?
        } else {
            session
        };
        let resp = self.post(LOGOUT_PATH, Some(&session.access_token), None)?;
        if resp.status().is_success() || is_session_gone(resp.status()) {
            Ok(())
        } else {
            Err(SyncError::Auth(format!(
                "sign-out failed: HTTP {}",
                resp.status()
            )))
        }
    }

    /// [`Self::sign_out`] on a background thread, so a caller on the UI
    /// thread never waits for the network. Best effort: the handle is only
    /// there for tests to wait on.
    pub fn sign_out_in_background(&self, session: Session) -> JoinHandle<Result<(), SyncError>> {
        let auth = self.clone();
        thread::spawn(move || auth.sign_out(session))
    }

    fn call(&self, path: &str, body: serde_json::Value) -> Result<Session, SyncError> {
        let resp = self.post(path, None, Some(body))?;
        self.session_from(resp)
    }

    fn post(
        &self,
        path: &str,
        bearer: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::blocking::Response, SyncError> {
        let url = format!("{}{path}", self.base_url);
        let mut request = self.client.post(url).header("apikey", &self.anon_key);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().map_err(|e| SyncError::Auth(e.to_string()))
    }

    fn session_from(&self, resp: reqwest::blocking::Response) -> Result<Session, SyncError> {
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
            email: raw.user.email,
            expires_at: self.clock.now() + raw.expires_in,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use todo_core::FixedClock;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn session_with_expiry(expires_at: i64) -> Session {
        Session {
            access_token: "old-access".to_string(),
            refresh_token: "old-refresh".to_string(),
            user_id: "user-789".to_string(),
            email: None,
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
        assert_eq!(session.email.as_deref(), Some("a@example.com"));
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
        // The stored session had none (stored before emails were kept);
        // the refresh fills it in.
        assert_eq!(result.email.as_deref(), Some("a@example.com"));
    }

    fn response_without_email() -> serde_json::Value {
        let mut response = canned_response();
        response["user"] = serde_json::json!({ "id": "user-789" });
        response
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refresh_without_an_email_keeps_the_one_the_session_had() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_without_email()))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = Session {
            email: Some("old@example.com".to_string()),
            ..session_with_expiry(-1)
        };
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0))).ensure_fresh(session)
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.access_token, "access-123");
        assert_eq!(result.email.as_deref(), Some("old@example.com"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_response_without_an_email_gives_a_session_without_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_without_email()))
            .mount(&server)
            .await;

        let base = server.uri();
        let session = tokio::task::spawn_blocking(move || {
            AuthClient::new(base, "anon-key").sign_in("a@example.com", "hunter2")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(session.user_id, "user-789");
        assert_eq!(session.email, None);
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

    /// Signs `session` out against `server` with the clock at 0, on a
    /// blocking thread (the client is `reqwest::blocking`).
    async fn sign_out(base: String, session: Session) -> Result<(), SyncError> {
        tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0))).sign_out(session)
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_revokes_only_this_devices_session() {
        let server = MockServer::start().await;
        // `scope=local` is what keeps the user's other devices signed in --
        // without it, Supabase defaults to revoking every session.
        Mock::given(method("POST"))
            .and(path("/auth/v1/logout"))
            .and(query_param("scope", "local"))
            .and(header("apikey", "anon-key"))
            .and(header("authorization", "Bearer old-access"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let result = sign_out(server.uri(), session_with_expiry(10_000)).await;

        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_refreshes_an_expired_session_and_revokes_it_with_the_new_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .and(query_param("grant_type", "refresh_token"))
            .and(body_json(
                serde_json::json!({ "refresh_token": "old-refresh" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/logout"))
            .and(query_param("scope", "local"))
            .and(header("authorization", "Bearer access-123"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let result = sign_out(server.uri(), session_with_expiry(-1)).await;

        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_with_a_rejected_refresh_token_is_already_done() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/logout"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let result = sign_out(server.uri(), session_with_expiry(-1)).await;

        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_fails_if_the_refresh_fails_for_another_reason() {
        for response in [
            ResponseTemplate::new(500),
            ResponseTemplate::new(200).set_body_string("not json"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/auth/v1/token"))
                .respond_with(response)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/auth/v1/logout"))
                .respond_with(ResponseTemplate::new(204))
                .expect(0)
                .mount(&server)
                .await;

            let result = sign_out(server.uri(), session_with_expiry(-1)).await;

            assert!(matches!(result, Err(SyncError::Auth(_))));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_of_a_session_the_server_no_longer_knows_is_already_done() {
        for status in [401, 403, 404] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/auth/v1/logout"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let result = sign_out(server.uri(), session_with_expiry(10_000)).await;

            assert!(result.is_ok(), "HTTP {status}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_rejected_for_another_reason_is_an_error() {
        // 400 included: from the logout endpoint it means *our* request was
        // wrong (e.g. an unsupported scope), not that the session is gone.
        for status in [400, 500] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/auth/v1/logout"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let result = sign_out(server.uri(), session_with_expiry(10_000)).await;

            assert!(matches!(result, Err(SyncError::Auth(_))), "HTTP {status}");
        }
    }

    #[test]
    fn sign_out_with_the_server_unreachable_is_an_error() {
        // Port 1 on loopback: refused immediately, no DNS or timeout involved.
        let auth =
            AuthClient::with_clock("http://127.0.0.1:1", "anon-key", Arc::new(FixedClock(0)));

        // Fresh (straight to logout) and expired (fails at the refresh).
        for expires_at in [10_000, -1] {
            let result = auth.sign_out(session_with_expiry(expires_at));

            assert!(
                matches!(result, Err(SyncError::Auth(_))),
                "expires_at {expires_at}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_out_in_background_revokes_the_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/logout"))
            .and(query_param("scope", "local"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let base = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::with_clock(base, "anon-key", Arc::new(FixedClock(0)))
                .sign_out_in_background(session_with_expiry(10_000))
                .join()
                .unwrap()
        })
        .await
        .unwrap();

        assert!(result.is_ok());
    }

    /// A 307 makes reqwest re-send the POST body, here the password, to the
    /// redirect target, so redirects must not be followed at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_redirect_is_not_followed_and_the_credentials_stay_put() {
        let elsewhere = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .expect(0)
            .mount(&elsewhere)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/steal", elsewhere.uri())),
            )
            .mount(&server)
            .await;

        let base = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            AuthClient::new(base, "anon-key").sign_in("a@example.com", "hunter2")
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(SyncError::Auth(_))));
    }

    /// Tests run with `https_only` off so they can reach wiremock; this pins
    /// down that the production client refuses plain http before sending.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_production_client_refuses_plain_http() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(canned_response()))
            .expect(0)
            .mount(&server)
            .await;

        let base = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            let mut auth = AuthClient::new(base, "anon-key");
            auth.client = crate::build_http_client(true);
            auth.sign_in("a@example.com", "hunter2")
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(SyncError::Auth(_))));
    }
}
