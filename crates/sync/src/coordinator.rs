//! Automatic sync triggers: local edits settling, a periodic fallback (so a
//! device picks up other devices' changes even with no local edits of its
//! own), and an explicit "sync soon" for platform lifecycle events (app
//! foreground, the Mac waking, right after signing in) that shouldn't wait
//! for either of the other two.
//!
//! Deliberately reuses `App::subscribe` rather than adding anything new to
//! `todo-core` — `todo-core` has no idea sync exists; this crate just
//! listens to the same "something changed" signal the UI already does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use todo_core::App;

use crate::auth::Session;
use crate::engine::{SyncEngine, SyncOutcome};
use crate::SyncError;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy)]
pub struct CoordinatorConfig {
    /// How long local changes must sit untouched before they're worth
    /// pushing. Independent of (though matching, by default) `todo-core`'s
    /// own disk-flush debounce — the two timers don't coordinate, they just
    /// happen to agree on a reasonable window.
    pub debounce: Duration,
    /// Fallback cadence so a device with no local edits of its own still
    /// eventually picks up other devices' changes.
    pub periodic: Duration,
    /// How often the background loop wakes to check whether a sync is due.
    pub poll: Duration,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(2),
            periodic: Duration::from_secs(5 * 60),
            poll: Duration::from_millis(200),
        }
    }
}

#[allow(clippy::type_complexity)]
struct State {
    app: Arc<App>,
    engine: SyncEngine,
    session: Mutex<Option<Session>>,
    dirty_since: Mutex<Option<Instant>>,
    last_attempt: Mutex<Option<Instant>>,
    force: AtomicBool,
    shutdown: AtomicBool,
    config: CoordinatorConfig,
    on_result: Mutex<Option<Arc<dyn Fn(Result<SyncOutcome, SyncError>) + Send + Sync>>>,
    on_session_refreshed: Mutex<Option<Arc<dyn Fn(Session) + Send + Sync>>>,
}

impl State {
    fn due(&self) -> bool {
        if lock(&self.session).is_none() {
            return false;
        }
        if self.force.load(Ordering::SeqCst) {
            return true;
        }
        let now = Instant::now();
        let debounce_elapsed =
            lock(&self.dirty_since).is_some_and(|t| now.duration_since(t) >= self.config.debounce);
        // `None` (never attempted) counts as elapsed, not "not yet due" --
        // this is what makes a device sync promptly right after signing in
        // (or on first launch with an already-stored session) rather than
        // waiting a full `periodic` window for its first attempt.
        let periodic_elapsed =
            lock(&self.last_attempt).is_none_or(|t| now.duration_since(t) >= self.config.periodic);
        debounce_elapsed || periodic_elapsed
    }

    fn attempt_sync(&self) {
        self.force.store(false, Ordering::SeqCst);
        *lock(&self.dirty_since) = None;
        *lock(&self.last_attempt) = Some(Instant::now());
        let Some(original) = lock(&self.session).clone() else {
            return;
        };
        let (session, result) = self.engine.sync_once(&self.app, original.clone());
        // Reported before the sync result below -- Supabase invalidates the
        // old refresh token the instant a new one is issued, so a rotated
        // session must reach the listener even if `result` then turns out to
        // be an error, or the caller is left holding a refresh token that no
        // longer works. But only if it was actually rotated *and* adopted:
        // see `adopt_refreshed` for why a sign-out mid-sync must win.
        if session != original && self.adopt_refreshed(&original, session.clone()) {
            if let Some(on_refreshed) = lock(&self.on_session_refreshed).clone() {
                on_refreshed(session);
            }
        }
        if let Some(listener) = lock(&self.on_result).clone() {
            listener(result);
        }
    }

    /// Compare-and-swap: replaces the stored session with `refreshed` only
    /// if it is still `original`, the session the refresh started from.
    /// Anything else means the caller changed it mid-sync -- signed out
    /// (`None`) or signed in as someone else -- and writing the refreshed
    /// one back would silently undo that, resurrecting a session the user
    /// just got rid of. Returns whether `refreshed` was adopted, i.e.
    /// whether it is safe to persist.
    fn adopt_refreshed(&self, original: &Session, refreshed: Session) -> bool {
        let mut current = lock(&self.session);
        if current.as_ref() != Some(original) {
            return false;
        }
        *current = Some(refreshed);
        true
    }
}

/// Owns the background thread; dropping it stops the loop and joins the
/// thread, mirroring `todo_core::App`'s own writer-thread lifecycle.
pub struct AutoSyncCoordinator {
    state: Arc<State>,
    handle: Option<JoinHandle<()>>,
}

impl AutoSyncCoordinator {
    pub fn new(app: Arc<App>, engine: SyncEngine) -> Arc<Self> {
        Self::with_config(app, engine, CoordinatorConfig::default())
    }

    pub fn with_config(app: Arc<App>, engine: SyncEngine, config: CoordinatorConfig) -> Arc<Self> {
        let state = Arc::new(State {
            app: Arc::clone(&app),
            engine,
            session: Mutex::new(None),
            dirty_since: Mutex::new(None),
            last_attempt: Mutex::new(None),
            force: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            config,
            on_result: Mutex::new(None),
            on_session_refreshed: Mutex::new(None),
        });

        // Any change at all -- a local edit, or even a change this same
        // coordinator just pulled -- marks the doc dirty. A debounce tick
        // that finds nothing new to push after a pull-triggered wakeup is a
        // cheap no-op (`App::has_unpushed_changes` is false by then), not a
        // real waste, so there's no need to distinguish the two sources.
        // `subscribe` also fires once immediately with the current state,
        // so a coordinator created for an already-signed-in device marks
        // itself dirty right away too -- a deliberate side effect, not a
        // bug: it means a relaunch with an existing session syncs shortly
        // after startup rather than waiting for the first edit.
        let dirty_state = Arc::clone(&state);
        app.subscribe(move |_snapshot| {
            *lock(&dirty_state.dirty_since) = Some(Instant::now());
        });

        let loop_state = Arc::clone(&state);
        let poll = config.poll;
        let handle = thread::spawn(move || loop {
            thread::sleep(poll);
            if loop_state.shutdown.load(Ordering::SeqCst) {
                break;
            }
            if loop_state.due() {
                loop_state.attempt_sync();
            }
        });

        Arc::new(Self {
            state,
            handle: Some(handle),
        })
    }

    /// `None` pauses syncing entirely (e.g. signed out) without tearing
    /// down the background thread.
    pub fn set_session(&self, session: Option<Session>) {
        *lock(&self.state.session) = session;
    }

    /// For a refresh that happened outside this coordinator (a manual "Sync
    /// Now" round): adopts `refreshed` as the session auto-sync uses, but
    /// only if the coordinator still holds `original` -- see
    /// `State::adopt_refreshed`. `false` means the session changed in the
    /// meantime (signed out, or a different account), so the caller must
    /// not persist `refreshed` either.
    pub fn adopt_refreshed(&self, original: &Session, refreshed: Session) -> bool {
        self.state.adopt_refreshed(original, refreshed)
    }

    /// Requests an immediate sync attempt, bypassing the debounce/periodic
    /// wait — for platform lifecycle events (app foreground, the Mac waking
    /// from sleep) and right after signing in, where waiting for the normal
    /// cadence would feel broken rather than just eventually consistent.
    pub fn sync_soon(&self) {
        self.state.force.store(true, Ordering::SeqCst);
    }

    pub fn set_listener(
        &self,
        on_result: impl Fn(Result<SyncOutcome, SyncError>) + Send + Sync + 'static,
    ) {
        *lock(&self.state.on_result) = Some(Arc::new(on_result));
    }

    /// Called whenever a sync attempt rotates the session's tokens --
    /// independent of whether the sync attempt itself then succeeds, so the
    /// caller can persist it (Keychain) promptly. See `State::attempt_sync`'s
    /// doc comment for why "independent of" matters here.
    pub fn set_session_listener(
        &self,
        on_session_refreshed: impl Fn(Session) + Send + Sync + 'static,
    ) {
        *lock(&self.state.on_session_refreshed) = Some(Arc::new(on_session_refreshed));
    }
}

impl Drop for AutoSyncCoordinator {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::mpsc;

    use tempfile::TempDir;
    use todo_core::{Command, FixedClock};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::auth::AuthClient;
    use crate::transport::{InMemoryTransport, SyncTransport};

    fn open(dir: &TempDir, name: &str) -> Arc<App> {
        Arc::new(App::open(dir.path().join(name).to_str().unwrap()).unwrap())
    }

    fn add(app: &App, title: &str) {
        app.dispatch(Command::Add {
            title: title.to_string(),
            after: None,
            due: None,
            list_id: None,
        })
        .unwrap();
    }

    /// Never needs a refresh -- tests that only care about the pull/push
    /// scheduling never touch the network via `AuthClient` at all.
    fn session(token: &str) -> Session {
        Session {
            access_token: token.to_string(),
            refresh_token: "refresh".to_string(),
            user_id: "user".to_string(),
            expires_at: i64::MAX,
        }
    }

    /// A placeholder `AuthClient` for tests whose session never needs
    /// refreshing -- its base URL is never actually requested.
    fn unused_auth() -> AuthClient {
        AuthClient::new("http://unused.invalid", "anon-key")
    }

    /// Short enough to keep tests fast, long enough to be reliably
    /// distinguishable from scheduling jitter on a loaded machine.
    fn fast_config() -> CoordinatorConfig {
        CoordinatorConfig {
            debounce: Duration::from_millis(40),
            periodic: Duration::from_millis(200),
            poll: Duration::from_millis(5),
        }
    }

    #[test]
    fn without_a_session_nothing_ever_syncs() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth()),
            fast_config(),
        );

        thread::sleep(Duration::from_millis(300)); // well past debounce + periodic
        assert!(transport.pull("tok", None).unwrap().is_empty());
        drop(coordinator);
    }

    #[test]
    fn a_local_edit_is_pushed_after_the_debounce_window() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth()),
            fast_config(),
        );
        coordinator.set_session(Some(session("tok")));

        add(&app, "local task");
        thread::sleep(Duration::from_millis(150)); // > debounce, < periodic
        assert_eq!(transport.pull("tok", None).unwrap().len(), 1);
        drop(coordinator);
    }

    #[test]
    fn periodic_fallback_pulls_even_with_no_local_edits() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());

        // Debounce deliberately much longer than the test's sleeps, so the
        // only thing that can trigger a sync here — including the dirty
        // mark `App::subscribe`'s own immediate first fire leaves behind —
        // is the periodic fallback, not the debounce path already covered
        // by the test above.
        let config = CoordinatorConfig {
            debounce: Duration::from_secs(10),
            periodic: Duration::from_millis(60),
            poll: Duration::from_millis(5),
        };
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth()),
            config,
        );
        coordinator.set_session(Some(session("tok")));

        // The very first check is always immediately "due" (nothing
        // attempted yet) -- that's deliberate (see `State::due`'s doc
        // comment on why), but it means letting that one-off settle first
        // is what makes the assertion below prove the interval genuinely
        // *repeats*, not just that a device syncs once on its first check.
        thread::sleep(Duration::from_millis(30));
        // Real Loro update bytes, not an arbitrary string -- `import_from_pull`
        // rejects anything else, and a failed import with no listener
        // registered fails silently, which is exactly the trap this once
        // fell into while writing this test.
        let remote = open(&dir, "remote.sqlite3");
        add(&remote, "from elsewhere");
        transport
            .push("tok", remote.peer_id(), remote.export_for_push().unwrap())
            .unwrap();

        thread::sleep(Duration::from_millis(150)); // several periodic ticks
                                                   // Not asserting an exact `last_pulled_seq`: once the remote row is
                                                   // pulled in, `app`'s own `pushed_vv` doesn't yet cover those
                                                   // foreign ops, so it re-announces them as its own push next round
                                                   // — and the round after *that* pulls its own re-announcement back
                                                   // (harmless, since Loro import is idempotent, but it does advance
                                                   // the seq cursor further than a naive "one row in, one row out"
                                                   // count). What actually matters is that the content arrived with
                                                   // no local edit ever having happened on `app`.
        assert!(app
            .current()
            .rows
            .iter()
            .any(|r| r.title == "from elsewhere"));
        drop(coordinator);
    }

    #[test]
    fn sync_soon_bypasses_the_debounce_wait() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth()),
            fast_config(),
        );
        coordinator.set_session(Some(session("tok")));

        add(&app, "local task");
        coordinator.sync_soon();
        thread::sleep(Duration::from_millis(20)); // well under the 40ms debounce
        assert_eq!(transport.pull("tok", None).unwrap().len(), 1);
        drop(coordinator);
    }

    #[test]
    fn set_session_none_pauses_syncing() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth()),
            fast_config(),
        );
        coordinator.set_session(Some(session("tok")));
        coordinator.set_session(None);

        add(&app, "local task");
        coordinator.sync_soon();
        thread::sleep(Duration::from_millis(150));
        assert!(transport.pull("tok", None).unwrap().is_empty());
        drop(coordinator);
    }

    #[test]
    fn listener_is_called_with_the_sync_outcome() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth()),
            fast_config(),
        );

        let (tx, rx) = mpsc::channel();
        coordinator.set_listener(move |result| tx.send(result).unwrap());
        coordinator.set_session(Some(session("tok")));
        coordinator.sync_soon();

        let outcome = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert!(outcome.pushed_bytes > 0);
        drop(coordinator);
    }

    #[test]
    fn new_uses_the_default_config_and_still_syncs_via_sync_soon() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::new(
            Arc::clone(&app),
            SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth()),
        );
        coordinator.set_session(Some(session("tok")));

        // `sync_soon` bypasses the default config's multi-second debounce
        // entirely, so this doesn't need to wait anywhere near that long —
        // only past the default 200ms poll interval.
        coordinator.sync_soon();
        thread::sleep(Duration::from_millis(400));
        assert_eq!(transport.pull("tok", None).unwrap().len(), 1);
        drop(coordinator);
    }

    #[test]
    fn attempt_sync_with_no_session_is_a_no_op_not_a_panic() {
        // Exercises `State::attempt_sync`'s own defensive no-session guard
        // directly -- in practice `due()` already prevents the background
        // loop from ever calling this without a session, but a concurrent
        // `set_session(None)` landing between that check and this call is a
        // real (if narrow) race in production, so the guard stays and gets
        // tested at the unit it actually protects.
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        let state = State {
            app,
            engine: SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth()),
            session: Mutex::new(None),
            dirty_since: Mutex::new(None),
            last_attempt: Mutex::new(None),
            force: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            config: fast_config(),
            on_result: Mutex::new(None),
            on_session_refreshed: Mutex::new(None),
        };

        state.attempt_sync(); // must not panic
        assert!(lock(&state.last_attempt).is_some()); // still records the attempt time
    }

    /// A transport whose pull announces it has started, then stalls --
    /// stands in for a slow network round-trip, so a test can change the
    /// session while a sync is provably in flight.
    struct StallingTransport {
        started: Mutex<mpsc::Sender<()>>,
        stall: Duration,
    }

    impl SyncTransport for StallingTransport {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Ok(())
        }

        fn pull(&self, _: &str, _: Option<i64>) -> Result<Vec<crate::PulledUpdate>, SyncError> {
            let _ = lock(&self.started).send(());
            thread::sleep(self.stall);
            Ok(vec![])
        }
    }

    fn stalling(stall: Duration) -> (Arc<StallingTransport>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let transport = StallingTransport {
            started: Mutex::new(tx),
            stall,
        };
        (Arc::new(transport), rx)
    }

    #[test]
    fn signing_out_mid_sync_is_not_undone_when_the_sync_finishes() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let (transport, started) = stalling(Duration::from_millis(100));
        let coordinator = AutoSyncCoordinator::with_config(
            Arc::clone(&app),
            SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth()),
            fast_config(),
        );
        let (tx, rx) = mpsc::channel();
        coordinator.set_session_listener(move |session| tx.send(session).unwrap());
        coordinator.set_session(Some(session("tok")));
        coordinator.sync_soon();

        started.recv_timeout(Duration::from_secs(2)).unwrap();
        coordinator.set_session(None); // the user signs out mid-sync

        // The in-flight sync finishing must neither resurrect the session
        // in the coordinator nor hand it back to be persisted.
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(*lock(&coordinator.state.session), None);
        drop(coordinator);
    }

    #[test]
    fn adopt_refreshed_replaces_the_session_only_if_it_is_still_the_original() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        let coordinator = AutoSyncCoordinator::with_config(
            app,
            SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth()),
            // Never syncs on its own during this test, so nothing but
            // `adopt_refreshed` itself touches the session.
            CoordinatorConfig {
                debounce: Duration::from_secs(60),
                periodic: Duration::from_secs(60),
                poll: Duration::from_millis(5),
            },
        );
        let original = session("old");
        let refreshed = session("new");

        // Signed out: nothing to adopt into.
        assert!(!coordinator.adopt_refreshed(&original, refreshed.clone()));
        assert_eq!(*lock(&coordinator.state.session), None);

        // A different session (another account) since the refresh started.
        let other = session("someone-else");
        coordinator.set_session(Some(other.clone()));
        assert!(!coordinator.adopt_refreshed(&original, refreshed.clone()));
        assert_eq!(*lock(&coordinator.state.session), Some(other));

        // Still the original: adopted.
        coordinator.set_session(Some(original.clone()));
        assert!(coordinator.adopt_refreshed(&original, refreshed.clone()));
        assert_eq!(*lock(&coordinator.state.session), Some(refreshed));
        drop(coordinator);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refresh_that_races_a_sign_out_is_not_reported() {
        let auth_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "refreshed-access",
                "refresh_token": "refreshed-refresh",
                "token_type": "bearer",
                "expires_in": 3600,
                "user": { "id": "user", "email": "a@example.com" }
            })))
            .mount(&auth_server)
            .await;

        // `spawn_blocking` for the same reason as the rotated-session test
        // below: the blocking `reqwest` client can't be dropped in async.
        let auth_uri = auth_server.uri();
        tokio::task::spawn_blocking(move || {
            let dir = TempDir::new().unwrap();
            let app = open(&dir, "a.sqlite3");
            let (transport, started) = stalling(Duration::from_millis(100));
            let auth = AuthClient::with_clock(auth_uri, "anon-key", Arc::new(FixedClock(0)));
            let coordinator = AutoSyncCoordinator::with_config(
                Arc::clone(&app),
                SyncEngine::new(transport as Arc<dyn SyncTransport>, auth),
                fast_config(),
            );
            let (tx, rx) = mpsc::channel();
            coordinator.set_session_listener(move |session| tx.send(session).unwrap());
            coordinator.set_session(Some(Session {
                expires_at: 0,
                ..session("stale-access")
            }));
            coordinator.sync_soon();

            // The refresh has already happened by the time pull starts.
            started.recv_timeout(Duration::from_secs(2)).unwrap();
            coordinator.set_session(None);

            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
            assert_eq!(*lock(&coordinator.state.session), None);
            drop(coordinator);
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rotated_session_is_persisted_via_the_session_listener_even_though_the_sync_result_is_reported_separately(
    ) {
        let auth_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "refreshed-access",
                "refresh_token": "refreshed-refresh",
                "token_type": "bearer",
                "expires_in": 3600,
                "user": { "id": "user-789", "email": "a@example.com" }
            })))
            .mount(&auth_server)
            .await;

        // `AuthClient` wraps a `reqwest::blocking::Client`, which owns its
        // own runtime under the hood -- constructing or dropping one (via
        // `coordinator` here) directly inside this test's async body panics
        // ("cannot drop a runtime in a context where blocking is not
        // allowed"), so the whole thing runs inside `spawn_blocking` instead,
        // same as every other blocking-client test in this crate.
        let auth_uri = auth_server.uri();
        tokio::task::spawn_blocking(move || {
            let dir = TempDir::new().unwrap();
            let app = open(&dir, "a.sqlite3");
            let transport = Arc::new(InMemoryTransport::new());
            let auth = AuthClient::with_clock(auth_uri, "anon-key", Arc::new(FixedClock(0)));
            let coordinator = AutoSyncCoordinator::with_config(
                Arc::clone(&app),
                SyncEngine::new(transport as Arc<dyn SyncTransport>, auth),
                fast_config(),
            );

            let (tx, rx) = mpsc::channel();
            coordinator.set_session_listener(move |session| tx.send(session).unwrap());
            let expiring = Session {
                expires_at: 0,
                ..session("stale-access")
            };
            coordinator.set_session(Some(expiring));
            coordinator.sync_soon();

            let refreshed = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(refreshed.access_token, "refreshed-access");
            drop(coordinator);
        })
        .await
        .unwrap();
    }
}
