//! `SyncTransport`: push/pull/compact over opaque bytes. `HttpTransport` is
//! the real Supabase-backed implementation; `InMemoryTransport` is a
//! test-only stand-in that follows the same rules as the server's
//! `sync_*` functions (`supabase/schema.sql`), so multi-device convergence
//! and compaction can be tested without any network at all — same "inject a
//! trait for testability" shape as `remember-core`'s `Clock`/`IdSource`
//! (`crates/core/src/clock.rs`).

use serde::Deserialize;

use crate::SyncError;

/// One row pulled from the server's append-only log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledUpdate {
    pub seq: i64,
    pub payload: Vec<u8>,
}

/// The account's full document as of `as_of_seq`: it subsumes every log row
/// up to and including that seq, and those rows no longer exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledSnapshot {
    pub as_of_seq: i64,
    pub payload: Vec<u8>,
}

/// One pull's worth of what a device is missing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullPage {
    /// Present only when the cursor pulled from is behind the snapshot, so
    /// the rows in between may be gone. Import it before `rows`.
    pub snapshot: Option<PulledSnapshot>,
    /// Rows after the cursor (or after the snapshot, whichever is later),
    /// oldest first.
    pub rows: Vec<PulledUpdate>,
    /// More rows exist past this page; pull again from the new cursor.
    pub has_more: bool,
    /// How many rows the log holds past the snapshot — what decides whether
    /// it's time to compact.
    pub log_len: i64,
}

pub trait SyncTransport: Send + Sync {
    /// Appends `payload` as a new row owned by whoever `token` authenticates
    /// as, tagged with the originating `device_id` (the pushing device's
    /// `App::peer_id`).
    fn push(&self, token: &str, device_id: u64, payload: Vec<u8>) -> Result<(), SyncError>;

    /// What a device whose last pulled seq is `since_seq` is missing. `None`
    /// means "never pulled".
    fn pull(&self, token: &str, since_seq: Option<i64>) -> Result<PullPage, SyncError>;

    /// Replaces the account's snapshot with `payload`, which must contain
    /// every row up to and including `as_of_seq`, and deletes those rows.
    /// `Ok(false)` if the server refused because `as_of_seq` is no longer
    /// one of the account's rows — another device compacted past it first.
    fn compact(&self, token: &str, as_of_seq: i64, payload: Vec<u8>) -> Result<bool, SyncError>;
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

/// Talks to a Supabase project through the `sync_push`/`sync_pull`/
/// `sync_compact` functions in `supabase/schema.sql`, never the tables
/// directly — see that file for why. `payload` is Postgres `bytea`, encoded
/// here in bytea's own hex text format (`\x...`) so it travels as a plain
/// JSON string with no extra encoding scheme of our own.
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
            client: crate::http_client(),
        }
    }

    fn rpc(
        &self,
        token: &str,
        function: &str,
        body: serde_json::Value,
    ) -> Result<reqwest::blocking::Response, SyncError> {
        let url = format!("{}/rest/v1/rpc/{function}", self.base_url);
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
                "{function} failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp)
    }
}

#[derive(Deserialize)]
struct RawRow {
    seq: i64,
    payload: String,
}

#[derive(Deserialize)]
struct RawSnapshot {
    as_of_seq: i64,
    payload: String,
}

#[derive(Deserialize)]
struct RawPage {
    snapshot: Option<RawSnapshot>,
    rows: Vec<RawRow>,
    has_more: bool,
    log_len: i64,
}

fn invalid_response(e: reqwest::Error) -> SyncError {
    SyncError::Transport(format!("invalid response: {e}"))
}

impl SyncTransport for HttpTransport {
    fn push(&self, token: &str, device_id: u64, payload: Vec<u8>) -> Result<(), SyncError> {
        self.rpc(
            token,
            "sync_push",
            serde_json::json!({
                "device_id": device_id as i64,
                "payload": to_hex(&payload),
            }),
        )?;
        Ok(())
    }

    fn pull(&self, token: &str, since_seq: Option<i64>) -> Result<PullPage, SyncError> {
        let raw: RawPage = self
            .rpc(
                token,
                "sync_pull",
                serde_json::json!({ "since": since_seq }),
            )?
            .json()
            .map_err(invalid_response)?;
        let snapshot = raw
            .snapshot
            .map(|s| {
                Ok::<_, SyncError>(PulledSnapshot {
                    as_of_seq: s.as_of_seq,
                    payload: from_hex(&s.payload)?,
                })
            })
            .transpose()?;
        let rows = raw
            .rows
            .into_iter()
            .map(|r| {
                Ok(PulledUpdate {
                    seq: r.seq,
                    payload: from_hex(&r.payload)?,
                })
            })
            .collect::<Result<_, SyncError>>()?;
        Ok(PullPage {
            snapshot,
            rows,
            has_more: raw.has_more,
            log_len: raw.log_len,
        })
    }

    fn compact(&self, token: &str, as_of_seq: i64, payload: Vec<u8>) -> Result<bool, SyncError> {
        self.rpc(
            token,
            "sync_compact",
            serde_json::json!({
                "payload": to_hex(&payload),
                "as_of_seq": as_of_seq,
            }),
        )?
        .json()
        .map_err(invalid_response)
    }
}

/// Test-only stand-in for the server, following the same rules as its
/// `sync_*` functions. Two `App`s pointed at the same `InMemoryTransport`
/// (behind an `Arc`) can prove multi-device convergence with no network
/// involved.
#[cfg(any(test, feature = "testing"))]
pub struct InMemoryTransport {
    state: std::sync::Mutex<InMemoryState>,
    page_limit: usize,
}

#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
struct InMemoryState {
    rows: Vec<PulledUpdate>,
    snapshot: Option<PulledSnapshot>,
    next_seq: i64,
}

#[cfg(any(test, feature = "testing"))]
impl Default for InMemoryTransport {
    fn default() -> Self {
        // `sync_pull`'s page size.
        Self::with_page_limit(1000)
    }
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pages smaller than the server's, so paging is cheap to test.
    pub fn with_page_limit(page_limit: usize) -> Self {
        Self {
            state: std::sync::Mutex::default(),
            page_limit,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, InMemoryState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The rows the log still holds, i.e. those no snapshot subsumes yet.
    pub fn log_rows(&self) -> Vec<PulledUpdate> {
        self.state().rows.clone()
    }

    pub fn snapshot(&self) -> Option<PulledSnapshot> {
        self.state().snapshot.clone()
    }
}

#[cfg(any(test, feature = "testing"))]
impl SyncTransport for InMemoryTransport {
    fn push(&self, _token: &str, _device_id: u64, payload: Vec<u8>) -> Result<(), SyncError> {
        let mut state = self.state();
        let seq = state.next_seq;
        state.next_seq += 1;
        state.rows.push(PulledUpdate { seq, payload });
        Ok(())
    }

    fn pull(&self, _token: &str, since_seq: Option<i64>) -> Result<PullPage, SyncError> {
        let state = self.state();
        let snapshot = state
            .snapshot
            .clone()
            .filter(|s| since_seq.is_none_or(|since| since < s.as_of_seq));
        let as_of_seq = state.snapshot.as_ref().map_or(-1, |s| s.as_of_seq);
        let after = since_seq.unwrap_or(-1).max(as_of_seq);
        let mut rows: Vec<PulledUpdate> = state
            .rows
            .iter()
            .filter(|r| r.seq > after)
            .cloned()
            .collect();
        let has_more = rows.len() > self.page_limit;
        rows.truncate(self.page_limit);
        Ok(PullPage {
            snapshot,
            rows,
            has_more,
            // Rows up to the snapshot are already gone.
            log_len: state.rows.len() as i64,
        })
    }

    fn compact(&self, _token: &str, as_of_seq: i64, payload: Vec<u8>) -> Result<bool, SyncError> {
        let mut state = self.state();
        if !state.rows.iter().any(|r| r.seq == as_of_seq) {
            return Ok(false);
        }
        state.snapshot = Some(PulledSnapshot { as_of_seq, payload });
        state.rows.retain(|r| r.seq > as_of_seq);
        Ok(true)
    }
}

#[cfg(test)]
type PullHook = Box<dyn Fn(&str) + Send + Sync>;
#[cfg(test)]
type PullAnswer = Box<dyn Fn() -> Result<PullPage, SyncError> + Send + Sync>;

/// An [`InMemoryTransport`] with a failure or a hook spliced into one of
/// its calls. Tests share this one double rather than each writing its own,
/// so a test spells out only how its transport differs from a working one.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct ScriptedTransport {
    pub inner: InMemoryTransport,
    /// Runs as each pull is on the wire, with that pull's token.
    pub during_pull: Option<PullHook>,
    /// Answers each pull in place of `inner`.
    pub pull: Option<PullAnswer>,
    pub fail_push: bool,
    pub fail_compact: bool,
}

#[cfg(test)]
impl SyncTransport for ScriptedTransport {
    fn push(&self, token: &str, device_id: u64, payload: Vec<u8>) -> Result<(), SyncError> {
        if self.fail_push {
            return Err(SyncError::Transport("push failed".to_string()));
        }
        self.inner.push(token, device_id, payload)
    }

    fn pull(&self, token: &str, since_seq: Option<i64>) -> Result<PullPage, SyncError> {
        if let Some(during_pull) = &self.during_pull {
            during_pull(token);
        }
        match &self.pull {
            Some(pull) => pull(),
            None => self.inner.pull(token, since_seq),
        }
    }

    fn compact(&self, token: &str, as_of_seq: i64, payload: Vec<u8>) -> Result<bool, SyncError> {
        if self.fail_compact {
            return Err(SyncError::Transport("compact failed".to_string()));
        }
        self.inner.compact(token, as_of_seq, payload)
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

    fn payloads(page: &PullPage) -> Vec<&[u8]> {
        page.rows.iter().map(|r| r.payload.as_slice()).collect()
    }

    /// Pushes `payloads` in order to a fresh transport.
    fn with_rows(t: InMemoryTransport, payloads: &[&[u8]]) -> InMemoryTransport {
        for p in payloads {
            t.push("tok", 1, p.to_vec()).unwrap();
        }
        t
    }

    #[test]
    fn in_memory_transport_pull_with_no_cursor_returns_everything() {
        let t = with_rows(InMemoryTransport::new(), &[b"a", b"b"]);

        let page = t.pull("tok", None).unwrap();
        assert_eq!(payloads(&page), vec![b"a", b"b"]);
        assert_eq!(page.snapshot, None);
        assert!(!page.has_more);
        assert_eq!(page.log_len, 2);
    }

    #[test]
    fn in_memory_transport_pull_respects_cursor() {
        let t = with_rows(InMemoryTransport::new(), &[b"a", b"b"]);
        assert_eq!(payloads(&t.pull("tok", Some(0)).unwrap()), vec![b"b"]);
    }

    #[test]
    fn in_memory_transport_pull_past_the_end_is_empty() {
        let t = with_rows(InMemoryTransport::new(), &[b"a"]);
        assert!(t.pull("tok", Some(0)).unwrap().rows.is_empty());
    }

    #[test]
    fn in_memory_transport_survives_a_panic_while_its_state_is_locked() {
        let t = with_rows(InMemoryTransport::new(), &[b"a"]);
        let _ = std::panic::catch_unwind(|| {
            let _state = t.state.lock().unwrap();
            panic!("poisons the state mutex");
        });
        assert!(t.state.is_poisoned());

        assert_eq!(t.log_rows().len(), 1);
    }

    #[test]
    fn scripted_transport_with_nothing_scripted_is_its_inner_transport() {
        let t = ScriptedTransport::default();
        t.push("tok", 1, b"a".to_vec()).unwrap();

        assert_eq!(payloads(&t.pull("tok", None).unwrap()), vec![b"a"]);
        assert!(t.compact("tok", 0, b"snap".to_vec()).unwrap());
        assert_eq!(t.inner.snapshot().unwrap().as_of_seq, 0);
    }

    #[test]
    fn in_memory_transport_pages_past_its_limit() {
        let t = with_rows(InMemoryTransport::with_page_limit(2), &[b"a", b"b", b"c"]);

        let first = t.pull("tok", None).unwrap();
        assert_eq!(payloads(&first), vec![b"a", b"b"]);
        assert!(first.has_more);
        assert_eq!(first.log_len, 3);

        let second = t.pull("tok", Some(1)).unwrap();
        assert_eq!(payloads(&second), vec![b"c"]);
        assert!(!second.has_more);
    }

    #[test]
    fn in_memory_transport_compaction_replaces_the_rows_it_covers() {
        let t = with_rows(InMemoryTransport::new(), &[b"a", b"b", b"c"]);

        assert!(t.compact("tok", 1, b"ab".to_vec()).unwrap());

        assert_eq!(
            t.snapshot(),
            Some(PulledSnapshot {
                as_of_seq: 1,
                payload: b"ab".to_vec()
            })
        );
        assert_eq!(t.log_rows().len(), 1);
        // Seqs keep counting up past deleted rows.
        t.push("tok", 1, b"d".to_vec()).unwrap();
        assert_eq!(t.log_rows().last().unwrap().seq, 3);
    }

    #[test]
    fn in_memory_transport_hands_a_device_behind_the_snapshot_the_snapshot_and_the_tail() {
        let t = with_rows(InMemoryTransport::new(), &[b"a", b"b", b"c"]);
        t.compact("tok", 1, b"ab".to_vec()).unwrap();

        for since in [None, Some(0)] {
            let page = t.pull("tok", since).unwrap();
            assert_eq!(page.snapshot.as_ref().unwrap().payload, b"ab");
            assert_eq!(payloads(&page), vec![b"c"]);
            assert_eq!(page.log_len, 1);
        }
    }

    #[test]
    fn in_memory_transport_skips_the_snapshot_for_a_device_already_past_it() {
        let t = with_rows(InMemoryTransport::new(), &[b"a", b"b", b"c"]);
        t.compact("tok", 1, b"ab".to_vec()).unwrap();

        for since in [Some(1), Some(2)] {
            assert_eq!(t.pull("tok", since).unwrap().snapshot, None);
        }
    }

    #[test]
    fn in_memory_transport_refuses_a_compaction_at_a_seq_it_no_longer_holds() {
        let t = with_rows(InMemoryTransport::new(), &[b"a", b"b", b"c"]);
        t.compact("tok", 1, b"ab".to_vec()).unwrap();

        // Stale: another device already compacted past seq 0.
        assert!(!t.compact("tok", 0, b"a".to_vec()).unwrap());
        // Never existed.
        assert!(!t.compact("tok", 99, b"?".to_vec()).unwrap());
        assert_eq!(t.snapshot().unwrap().as_of_seq, 1);
        assert_eq!(t.log_rows().len(), 1);
    }

    mod http {
        use super::*;
        use wiremock::matchers::{body_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        /// Runs `call` against a fresh `HttpTransport` pointed at `server`
        /// — off the async runtime, since the transport is blocking.
        async fn call<T: Send + 'static>(
            server: &MockServer,
            call: impl FnOnce(HttpTransport) -> T + Send + 'static,
        ) -> T {
            let base = server.uri();
            tokio::task::spawn_blocking(move || call(HttpTransport::new(base, "anon-key")))
                .await
                .unwrap()
        }

        async fn respond(rpc: &str, response: ResponseTemplate) -> MockServer {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(format!("/rest/v1/rpc/{rpc}")))
                .respond_with(response)
                .mount(&server)
                .await;
            server
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn push_calls_sync_push_with_hex_payload_and_auth_headers() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/rest/v1/rpc/sync_push"))
                .and(header("apikey", "anon-key"))
                .and(header("authorization", "Bearer tok"))
                .and(body_json(serde_json::json!({
                    "device_id": 42,
                    "payload": "\\x0102"
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(7))
                .mount(&server)
                .await;

            call(&server, |t| t.push("tok", 42, vec![1, 2]))
                .await
                .unwrap();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn push_sends_a_peer_id_above_i64_max_as_its_bit_pattern() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/rest/v1/rpc/sync_push"))
                .and(body_json(serde_json::json!({
                    "device_id": -1,
                    "payload": "\\x"
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(7))
                .mount(&server)
                .await;

            call(&server, |t| t.push("tok", u64::MAX, vec![]))
                .await
                .unwrap();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_non_success_status_is_a_transport_error_naming_the_function() {
            let server = respond("sync_push", ResponseTemplate::new(500)).await;

            let result = call(&server, |t| t.push("tok", 1, vec![1])).await;

            assert!(
                matches!(result, Err(SyncError::Transport(m)) if m == "sync_push failed: HTTP 500 Internal Server Error")
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_unreachable_server_is_a_transport_error() {
            // Port 1 on loopback: refused immediately. A dropped `MockServer`
            // would not do -- wiremock pools them, so its port keeps answering.
            let result = tokio::task::spawn_blocking(|| {
                HttpTransport::new("http://127.0.0.1:1", "anon-key").pull("tok", None)
            })
            .await
            .unwrap();

            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_with_no_cursor_sends_a_null_since() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/rest/v1/rpc/sync_pull"))
                .and(header("apikey", "anon-key"))
                .and(header("authorization", "Bearer tok"))
                .and(body_json(serde_json::json!({ "since": null })))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "snapshot": null,
                    "rows": [],
                    "has_more": false,
                    "log_len": 0
                })))
                .mount(&server)
                .await;

            let page = call(&server, |t| t.pull("tok", None)).await.unwrap();
            assert_eq!(page, PullPage::default());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_decodes_snapshot_rows_and_paging_fields() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/rest/v1/rpc/sync_pull"))
                .and(body_json(serde_json::json!({ "since": 7 })))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "snapshot": { "payload": "\\x0a0b", "as_of_seq": 9 },
                    "rows": [
                        { "seq": 10, "payload": "\\x01" },
                        { "seq": 12, "payload": "\\x0203" }
                    ],
                    "has_more": true,
                    "log_len": 1500
                })))
                .mount(&server)
                .await;

            let page = call(&server, |t| t.pull("tok", Some(7))).await.unwrap();

            assert_eq!(
                page,
                PullPage {
                    snapshot: Some(PulledSnapshot {
                        as_of_seq: 9,
                        payload: vec![10, 11]
                    }),
                    rows: vec![
                        PulledUpdate {
                            seq: 10,
                            payload: vec![1]
                        },
                        PulledUpdate {
                            seq: 12,
                            payload: vec![2, 3]
                        },
                    ],
                    has_more: true,
                    log_len: 1500,
                }
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_malformed_body_is_transport_error() {
            let server = respond(
                "sync_pull",
                ResponseTemplate::new(200).set_body_string("not json"),
            )
            .await;

            let result = call(&server, |t| t.pull("tok", None)).await;
            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_rejects_a_row_with_invalid_hex_payload() {
            let server = respond(
                "sync_pull",
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "snapshot": null,
                    "rows": [{ "seq": 1, "payload": "\\xzz" }],
                    "has_more": false,
                    "log_len": 1
                })),
            )
            .await;

            let result = call(&server, |t| t.pull("tok", None)).await;
            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn pull_rejects_a_snapshot_with_invalid_hex_payload() {
            let server = respond(
                "sync_pull",
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "snapshot": { "payload": "\\xzz", "as_of_seq": 1 },
                    "rows": [],
                    "has_more": false,
                    "log_len": 0
                })),
            )
            .await;

            let result = call(&server, |t| t.pull("tok", None)).await;
            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn compact_calls_sync_compact_and_returns_its_verdict() {
            for verdict in [true, false] {
                let server = MockServer::start().await;
                Mock::given(method("POST"))
                    .and(path("/rest/v1/rpc/sync_compact"))
                    .and(header("apikey", "anon-key"))
                    .and(header("authorization", "Bearer tok"))
                    .and(body_json(serde_json::json!({
                        "payload": "\\x0102",
                        "as_of_seq": 5
                    })))
                    .respond_with(ResponseTemplate::new(200).set_body_json(verdict))
                    .mount(&server)
                    .await;

                let result = call(&server, |t| t.compact("tok", 5, vec![1, 2])).await;
                assert_eq!(result.unwrap(), verdict);
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn compact_non_success_status_is_transport_error() {
            let server = respond("sync_compact", ResponseTemplate::new(500)).await;

            let result = call(&server, |t| t.compact("tok", 5, vec![1])).await;
            assert!(matches!(result, Err(SyncError::Transport(_))));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn compact_with_a_non_boolean_body_is_transport_error() {
            let server = respond(
                "sync_compact",
                ResponseTemplate::new(200).set_body_string("not json"),
            )
            .await;

            let result = call(&server, |t| t.compact("tok", 5, vec![1])).await;
            assert!(matches!(result, Err(SyncError::Transport(_))));
        }
    }
}
