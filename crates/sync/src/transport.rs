//! `SyncTransport`: push/pull over opaque bytes. `HttpTransport` is the real
//! Supabase-backed implementation; `InMemoryTransport` is a test-only stand-in
//! so multi-device convergence can be tested without any network at all —
//! same "inject a trait for testability" shape as `todo-core`'s
//! `Clock`/`IdSource` (`crates/core/src/clock.rs`).

use serde::Deserialize;

use crate::SyncError;

/// One row pulled from the server's append-only log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledUpdate {
    pub seq: i64,
    pub payload: Vec<u8>,
}

pub trait SyncTransport: Send + Sync {
    /// Appends `payload` as a new row owned by whoever `token` authenticates
    /// as, tagged with the originating `device_id` (the pushing device's
    /// `App::peer_id`).
    fn push(&self, token: &str, device_id: u64, payload: Vec<u8>) -> Result<(), SyncError>;

    /// Rows after `since_seq` (exclusive), oldest first. `None` means "from
    /// the start of this account's log".
    fn pull(&self, token: &str, since_seq: Option<i64>) -> Result<Vec<PulledUpdate>, SyncError>;
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("\\x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Result<Vec<u8>, SyncError> {
    let s = s.strip_prefix("\\x").unwrap_or(s);
    if !s.len().is_multiple_of(2) {
        return Err(SyncError::Transport(format!(
            "odd-length hex payload ({} chars)",
            s.len()
        )));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| SyncError::Transport(format!("invalid hex payload: {e}")))
        })
        .collect()
}

/// Talks to a Supabase project: `sync_log` via PostgREST for push/pull.
/// `payload` is stored as Postgres `bytea`, encoded here in bytea's own
/// hex text format (`\x...`) so it travels as a plain JSON string with no
/// extra encoding scheme of our own — see the `sync_log` migration.
pub struct HttpTransport {
    base_url: String,
    anon_key: String,
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    pub fn new(base_url: impl Into<String>, anon_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            anon_key: anon_key.into(),
            client: reqwest::blocking::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct RawRow {
    seq: i64,
    payload: String,
}

impl SyncTransport for HttpTransport {
    fn push(&self, token: &str, device_id: u64, payload: Vec<u8>) -> Result<(), SyncError> {
        let url = format!("{}/rest/v1/sync_log", self.base_url);
        let body = serde_json::json!({
            "device_id": device_id as i64,
            "payload": to_hex(&payload),
        });
        let resp = self
            .client
            .post(url)
            .bearer_auth(token)
            .header("apikey", &self.anon_key)
            .json(&body)
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SyncError::Transport(format!(
                "push failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(())
    }

    fn pull(&self, token: &str, since_seq: Option<i64>) -> Result<Vec<PulledUpdate>, SyncError> {
        let since = since_seq.unwrap_or(-1);
        let url = format!(
            "{}/rest/v1/sync_log?seq=gt.{since}&order=seq.asc&select=seq,payload",
            self.base_url
        );
        let resp = self
            .client
            .get(url)
            .bearer_auth(token)
            .header("apikey", &self.anon_key)
            .send()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SyncError::Transport(format!(
                "pull failed: HTTP {}",
                resp.status()
            )));
        }
        let rows: Vec<RawRow> = resp
            .json()
            .map_err(|e| SyncError::Transport(format!("invalid pull response: {e}")))?;
        rows.into_iter()
            .map(|r| {
                Ok(PulledUpdate {
                    seq: r.seq,
                    payload: from_hex(&r.payload)?,
                })
            })
            .collect()
    }
}

/// Test-only stand-in for the server: a shared, ordered log in memory. Two
/// `App`s pointed at the same `InMemoryTransport` (behind an `Arc`) can
/// prove multi-device convergence with no network involved.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct InMemoryTransport {
    rows: std::sync::Mutex<Vec<PulledUpdate>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "testing"))]
impl SyncTransport for InMemoryTransport {
    fn push(&self, _token: &str, _device_id: u64, payload: Vec<u8>) -> Result<(), SyncError> {
        let mut rows = self.rows.lock().unwrap_or_else(|p| p.into_inner());
        let seq = rows.len() as i64;
        rows.push(PulledUpdate { seq, payload });
        Ok(())
    }

    fn pull(&self, _token: &str, since_seq: Option<i64>) -> Result<Vec<PulledUpdate>, SyncError> {
        let rows = self.rows.lock().unwrap_or_else(|p| p.into_inner());
        let start = since_seq.map(|s| s + 1).unwrap_or(0);
        Ok(rows.iter().filter(|r| r.seq >= start).cloned().collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = vec![0u8, 1, 2, 255, 16];
        let encoded = to_hex(&bytes);
        assert_eq!(encoded, "\\x000102ff10");
        assert_eq!(from_hex(&encoded).unwrap(), bytes);
    }

    #[test]
    fn from_hex_accepts_bytes_without_the_backslash_x_prefix() {
        assert_eq!(from_hex("0102ff").unwrap(), vec![1, 2, 255]);
    }

    #[test]
    fn from_hex_rejects_odd_length() {
        let err = from_hex("\\x0").err().unwrap();
        assert!(matches!(err, SyncError::Transport(_)));
    }

    #[test]
    fn from_hex_rejects_non_hex_characters() {
        let err = from_hex("\\xzz").err().unwrap();
        assert!(matches!(err, SyncError::Transport(_)));
    }

    #[test]
    fn empty_payload_round_trips() {
        assert_eq!(to_hex(&[]), "\\x");
        assert_eq!(from_hex("\\x").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn in_memory_transport_pull_with_no_cursor_returns_everything() {
        let t = InMemoryTransport::new();
        t.push("tok", 1, b"a".to_vec()).unwrap();
        t.push("tok", 1, b"b".to_vec()).unwrap();

        let rows = t.pull("tok", None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].payload, b"a");
        assert_eq!(rows[1].payload, b"b");
    }

    #[test]
    fn in_memory_transport_pull_respects_cursor() {
        let t = InMemoryTransport::new();
        t.push("tok", 1, b"a".to_vec()).unwrap();
        t.push("tok", 1, b"b".to_vec()).unwrap();

        let rows = t.pull("tok", Some(0)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload, b"b");
    }

    #[test]
    fn in_memory_transport_pull_past_the_end_is_empty() {
        let t = InMemoryTransport::new();
        t.push("tok", 1, b"a".to_vec()).unwrap();
        assert!(t.pull("tok", Some(0)).unwrap().is_empty());
    }

    mod http {
        use super::*;
        use wiremock::matchers::{body_json, header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn push_sends_hex_encoded_payload_with_auth_headers() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/rest/v1/sync_log"))
                .and(header("apikey", "anon-key"))
                .and(header("authorization", "Bearer tok"))
                .and(body_json(serde_json::json!({
                    "device_id": 42,
                    "payload": "\\x0102"
                })))
                .respond_with(ResponseTemplate::new(201))
                .mount(&server)
                .await;

            let base = server.uri();
            tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").push("tok", 42, vec![1, 2])
            })
            .await
            .unwrap()
            .unwrap();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn push_non_success_status_is_transport_error() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/rest/v1/sync_log"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;

            let base = server.uri();
            let result = tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").push("tok", 1, vec![1])
            })
            .await
            .unwrap();

            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_with_no_cursor_queries_from_minus_one() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/v1/sync_log"))
                .and(query_param("seq", "gt.-1"))
                .and(query_param("order", "seq.asc"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    { "seq": 1, "payload": "\\x0102" }
                ])))
                .mount(&server)
                .await;

            let base = server.uri();
            let rows = tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").pull("tok", None)
            })
            .await
            .unwrap()
            .unwrap();

            assert_eq!(
                rows,
                vec![PulledUpdate {
                    seq: 1,
                    payload: vec![1, 2]
                }]
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_with_cursor_queries_seq_greater_than_it() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/v1/sync_log"))
                .and(query_param("seq", "gt.7"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
                .mount(&server)
                .await;

            let base = server.uri();
            let rows = tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").pull("tok", Some(7))
            })
            .await
            .unwrap()
            .unwrap();

            assert!(rows.is_empty());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_non_success_status_is_transport_error() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/v1/sync_log"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;

            let base = server.uri();
            let result = tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").pull("tok", None)
            })
            .await
            .unwrap();

            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_malformed_body_is_transport_error() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/v1/sync_log"))
                .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
                .mount(&server)
                .await;

            let base = server.uri();
            let result = tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").pull("tok", None)
            })
            .await
            .unwrap();

            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_rejects_a_row_with_invalid_hex_payload() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/v1/sync_log"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    { "seq": 1, "payload": "\\xzz" }
                ])))
                .mount(&server)
                .await;

            let base = server.uri();
            let result = tokio::task::spawn_blocking(move || {
                HttpTransport::new(base, "anon-key").pull("tok", None)
            })
            .await
            .unwrap();

            assert!(matches!(result, Err(SyncError::Transport(_))));
        }
    }
}
