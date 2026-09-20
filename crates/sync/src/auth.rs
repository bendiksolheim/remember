//! Thin wrapper over Supabase's GoTrue auth REST API. Rust only ever
//! receives and returns a [`Session`] — persisting it (Keychain) and
//! refreshing it are the caller's job; see the working agreement's "no
//! business logic in Swift" exception for token storage specifically.

use serde::Deserialize;

use crate::SyncError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
}

pub struct AuthClient {
    base_url: String,
    anon_key: String,
    client: reqwest::blocking::Client,
}

#[derive(Deserialize)]
struct RawAuthResponse {
    access_token: String,
    refresh_token: String,
    user: RawUser,
}

#[derive(Deserialize)]
struct RawUser {
    id: String,
}

impl AuthClient {
    pub fn new(base_url: impl Into<String>, anon_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            anon_key: anon_key.into(),
            client: reqwest::blocking::Client::new(),
        }
    }

    pub fn sign_up(&self, email: &str, password: &str) -> Result<Session, SyncError> {
        self.call("/auth/v1/signup", email, password)
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<Session, SyncError> {
        self.call("/auth/v1/token?grant_type=password", email, password)
    }

    fn call(&self, path: &str, email: &str, password: &str) -> Result<Session, SyncError> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .client
            .post(url)
            .header("apikey", &self.anon_key)
            .json(&serde_json::json!({ "email": email, "password": password }))
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
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
}
