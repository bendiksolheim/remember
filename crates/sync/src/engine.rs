//! Orchestration: a session refresh, and the pull-then-merge-then-push
//! round. This is the one place that decides in what order to call `App`'s
//! sync primitives; deciding *when* to run a round, and owning the session,
//! is `AutoSyncCoordinator`'s job. `remember-ffi` must only delegate here, per
//! the FFI crate's own convention that decisions belong below it.

use std::sync::Arc;

use remember_core::App;

use crate::auth::{AuthClient, Session};
use crate::transport::SyncTransport;
use crate::SyncError;

/// Aborts a round whose account was swapped out from under it -- a sync
/// for a since-signed-out account still in flight when another account's
/// first round reset local state. Carrying on would import the old
/// account's pulled data into the new one's document, or push the new one's
/// data to the old account.
fn ensure_still_bound(app: &App, session: &Session) -> Result<(), SyncError> {
    if app.sync_account()?.as_deref() == Some(session.user_id.as_str()) {
        Ok(())
    } else {
        Err(SyncError::Auth(
            "account changed during sync; round abandoned".to_string(),
        ))
    }
}

/// Once the log holds more rows than this past the snapshot, a caught-up
/// device compacts it. Keeps every account at one snapshot plus a short
/// tail -- and a fresh install at one snapshot plus at most this many
/// updates to import -- however long the account has been in use.
const COMPACT_AFTER_ROWS: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncOutcome {
    pub pulled: usize,
    pub pushed_bytes: usize,
}

#[derive(Clone)]
pub struct SyncEngine {
    transport: Arc<dyn SyncTransport>,
    auth: AuthClient,
}

impl SyncEngine {
    pub fn new(transport: Arc<dyn SyncTransport>, auth: AuthClient) -> Self {
        Self { transport, auth }
    }

    /// Refreshes `session` if it's at or near expiry; otherwise hands it
    /// back unchanged. Supabase revokes the old refresh token the moment a
    /// new one is issued, so the caller must adopt and persist a rotated
    /// session straight away -- before any slow network work -- and must be
    /// the only one refreshing that session, or two copies end up fighting
    /// over the rotation. `AutoSyncCoordinator` is that one owner.
    pub fn refresh(&self, session: Session) -> Result<Session, SyncError> {
        self.auth.ensure_fresh(session)
    }

    /// One full sync round with an already-fresh `session` (see
    /// [`Self::refresh`]): pull first (so this device merges what other
    /// devices already contributed before contributing its own changes --
    /// this ordering doesn't affect correctness, Loro's merge is
    /// commutative either way, but it means a device's own push always
    /// reflects the latest merged state), then push, then compact the
    /// server's log if it has grown long enough.
    pub fn sync_once(&self, app: &App, session: &Session) -> Result<SyncOutcome, SyncError> {
        let token = session.access_token.as_str();
        // Resets local state first if a different account synced here
        // before -- see `App::bind_sync_account`.
        app.bind_sync_account(&session.user_id)?;

        let mut pulled = 0;
        let log_len = loop {
            let page = self.transport.pull(token, app.last_pulled_seq()?)?;
            // Before the rows: the rows between this device's cursor and
            // the snapshot are gone, and the snapshot stands in for them.
            if let Some(snapshot) = page.snapshot {
                ensure_still_bound(app, session)?;
                app.import_from_pull(&snapshot.payload)?;
                app.mark_pulled(snapshot.as_of_seq)?;
                pulled += 1;
            }
            let page_was_empty = page.rows.is_empty();
            for update in page.rows {
                ensure_still_bound(app, session)?;
                app.import_from_pull(&update.payload)?;
                app.mark_pulled(update.seq)?;
                pulled += 1;
            }
            // An empty page can't move the cursor, so asking again would
            // only get the same page back.
            if !page.has_more || page_was_empty {
                break page.log_len;
            }
        };

        ensure_still_bound(app, session)?;
        let pushed_bytes = if app.has_unpushed_changes()? {
            let bytes = app.export_for_push()?;
            self.transport.push(token, app.peer_id(), bytes.clone())?;
            app.mark_pushed()?;
            bytes.len()
        } else {
            0
        };
        if log_len > COMPACT_AFTER_ROWS {
            self.compact(app, session)?;
        }
        app.mark_synced()?;

        Ok(SyncOutcome {
            pulled,
            pushed_bytes,
        })
    }

    /// Replaces the server's log, up to this device's cursor, with a
    /// snapshot of its whole document -- which holds every row up to that
    /// cursor, having just pulled them. Best effort: if the server refuses
    /// (another device compacted further first) or the request fails, the
    /// log just stays a little longer and a later round tries again, so
    /// neither fails the round.
    fn compact(&self, app: &App, session: &Session) -> Result<(), SyncError> {
        let Some(as_of_seq) = app.last_pulled_seq()? else {
            return Ok(());
        };
        ensure_still_bound(app, session)?;
        let snapshot = app.export_for_compaction()?;
        let _ = self
            .transport
            .compact(&session.access_token, as_of_seq, snapshot);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use remember_core::Command;
    use tempfile::TempDir;

    use super::*;
    use crate::transport::InMemoryTransport;

    fn open(dir: &TempDir, name: &str) -> App {
        App::open(dir.path().join(name).to_str().unwrap()).unwrap()
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

    /// Never needs a refresh (`expires_at` is as far out as an `i64` goes),
    /// so tests that only care about the pull/push mechanics never touch
    /// the network via `AuthClient` at all.
    fn fresh_session(token: &str) -> Session {
        Session {
            access_token: token.to_string(),
            refresh_token: "refresh".to_string(),
            user_id: "user".to_string(),
            email: None,
            expires_at: i64::MAX,
        }
    }

    /// A placeholder `AuthClient` for tests whose session never needs
    /// refreshing -- its base URL is never actually requested.
    fn unused_auth() -> AuthClient {
        AuthClient::new("http://unused.invalid", "anon-key")
    }

    #[test]
    fn sync_once_on_a_brand_new_device_pushes_only_its_bootstrap_list() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport: Arc<dyn SyncTransport> = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport, unused_auth());

        // Even with no user edits yet, `App::open` already bootstrapped the
        // default list — see `remember_core::doc`'s module doc comment — so the
        // very first sync round pushes that, not nothing.
        let result = engine.sync_once(&app, &fresh_session("tok"));
        let outcome = result.unwrap();
        assert_eq!(outcome.pulled, 0);
        assert!(outcome.pushed_bytes > 0);
    }

    #[test]
    fn sync_once_pushes_local_changes() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task");
        let transport = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth());

        let result = engine.sync_once(&app, &fresh_session("tok"));
        assert!(result.unwrap().pushed_bytes > 0);
        assert_eq!(transport.log_rows().len(), 1);
        assert!(app.last_synced_at().unwrap().is_some());
    }

    #[test]
    fn sync_once_pulls_remote_changes_into_local_app() {
        let dir = TempDir::new().unwrap();
        let remote_writer = open(&dir, "writer.sqlite3");
        add(&remote_writer, "from elsewhere");
        let transport = Arc::new(InMemoryTransport::new());
        transport
            .push(
                "tok",
                remote_writer.peer_id(),
                remote_writer.export_for_push().unwrap(),
            )
            .unwrap();

        let app = open(&dir, "a.sqlite3");
        let engine = SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth());
        let result = engine.sync_once(&app, &fresh_session("tok"));

        let outcome = result.unwrap();
        assert_eq!(outcome.pulled, 1);
        assert_eq!(app.current().rows.len(), 1);
        assert_eq!(app.current().rows[0].title, "from elsewhere");
    }

    #[test]
    fn two_devices_converge_after_each_syncs_twice() {
        // Each device pushes its own change, then needs a second sync round
        // to pull the other's — a real client would do this on its normal
        // trigger cadence (foreground/timer/reconnect), not automatically.
        let dir = TempDir::new().unwrap();
        let device_a = open(&dir, "a.sqlite3");
        let device_b = open(&dir, "b.sqlite3");
        add(&device_a, "from a");
        add(&device_b, "from b");

        let transport = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth());

        engine.sync_once(&device_a, &fresh_session("tok")).unwrap();
        engine.sync_once(&device_b, &fresh_session("tok")).unwrap();
        // device_b's pull above already saw device_a's push. device_a still
        // needs a second round to see device_b's.
        engine.sync_once(&device_a, &fresh_session("tok")).unwrap();

        assert_eq!(device_a.current().rows.len(), 2);
        assert_eq!(device_b.current().rows.len(), 2);
    }

    struct PullFails;
    impl SyncTransport for PullFails {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Err(SyncError::Transport(
                "push should not be reached".to_string(),
            ))
        }
        fn pull(&self, _: &str, _: Option<i64>) -> Result<crate::PullPage, SyncError> {
            Err(SyncError::Transport("boom".to_string()))
        }
        fn compact(&self, _: &str, _: i64, _: Vec<u8>) -> Result<bool, SyncError> {
            unreachable!("compact after a failed pull")
        }
    }

    struct PushFails;
    impl SyncTransport for PushFails {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Err(SyncError::Transport("boom".to_string()))
        }
        fn pull(&self, _: &str, _: Option<i64>) -> Result<crate::PullPage, SyncError> {
            Ok(crate::PullPage::default())
        }
        fn compact(&self, _: &str, _: i64, _: Vec<u8>) -> Result<bool, SyncError> {
            unreachable!("compact after a failed push")
        }
    }

    #[test]
    fn sync_once_surfaces_a_pull_failure_without_pushing() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task");
        // `PullFails::push` errors differently, so reaching it would show.
        let engine = SyncEngine::new(Arc::new(PullFails), unused_auth());

        let result = engine.sync_once(&app, &fresh_session("tok"));
        assert!(matches!(result, Err(SyncError::Transport(m)) if m == "boom"));
        assert_eq!(app.last_synced_at().unwrap(), None);
    }

    #[test]
    fn sync_once_surfaces_a_push_failure_after_a_successful_pull() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task"); // ensures has_unpushed_changes() is true
        let engine = SyncEngine::new(Arc::new(PushFails), unused_auth());

        let result = engine.sync_once(&app, &fresh_session("tok"));
        assert!(matches!(result.err().unwrap(), SyncError::Transport(_)));
        // Half a round is not "synced".
        assert_eq!(app.last_synced_at().unwrap(), None);
    }

    fn session_for(user_id: &str) -> Session {
        Session {
            user_id: user_id.to_string(),
            ..fresh_session("tok")
        }
    }

    fn titles(app: &App) -> Vec<String> {
        app.current().rows.into_iter().map(|r| r.title).collect()
    }

    #[test]
    fn switching_accounts_never_carries_one_accounts_tasks_into_the_other() {
        let dir = TempDir::new().unwrap();
        let a_server = Arc::new(InMemoryTransport::new());
        let b_server = Arc::new(InMemoryTransport::new());
        let as_a = SyncEngine::new(a_server as Arc<dyn SyncTransport>, unused_auth());
        let as_b = SyncEngine::new(b_server.clone() as Arc<dyn SyncTransport>, unused_auth());

        // B already has a task, from another device.
        let b_elsewhere = open(&dir, "b-elsewhere.sqlite3");
        add(&b_elsewhere, "b's task");
        b_server
            .push(
                "tok",
                b_elsewhere.peer_id(),
                b_elsewhere.export_for_push().unwrap(),
            )
            .unwrap();

        // A uses this Mac first. Two rounds, so A's own push is pulled back
        // and A's cursor sits on the same seq as B's existing row -- the
        // row a stale cursor would wrongly skip.
        let app = open(&dir, "shared-mac.sqlite3");
        add(&app, "a's task");
        as_a.sync_once(&app, &session_for("a")).unwrap();
        as_a.sync_once(&app, &session_for("a")).unwrap();
        assert_eq!(app.last_pulled_seq().unwrap(), Some(0));

        // A signs out, B signs in on the same Mac.
        as_b.sync_once(&app, &session_for("b")).unwrap();
        assert_eq!(titles(&app), vec!["b's task".to_string()]);

        // ...and nothing of A's reached B's server.
        let fresh_b_device = open(&dir, "fresh-b.sqlite3");
        for update in b_server.log_rows() {
            fresh_b_device.import_from_pull(&update.payload).unwrap();
        }
        assert!(!titles(&fresh_b_device).contains(&"a's task".to_string()));
    }

    /// Stands in for another round (for a different account) resetting
    /// local state while this one's pull is still on the wire.
    struct AccountSwitchingTransport {
        app: Arc<App>,
        inner: InMemoryTransport,
    }

    impl SyncTransport for AccountSwitchingTransport {
        fn push(&self, token: &str, device_id: u64, payload: Vec<u8>) -> Result<(), SyncError> {
            self.inner.push(token, device_id, payload)
        }

        fn pull(&self, token: &str, since_seq: Option<i64>) -> Result<crate::PullPage, SyncError> {
            let page = self.inner.pull(token, since_seq)?;
            self.app.bind_sync_account("someone-else").unwrap();
            Ok(page)
        }

        fn compact(
            &self,
            token: &str,
            as_of_seq: i64,
            payload: Vec<u8>,
        ) -> Result<bool, SyncError> {
            self.inner.compact(token, as_of_seq, payload)
        }
    }

    #[test]
    fn a_round_whose_account_changes_mid_flight_is_abandoned() {
        let dir = TempDir::new().unwrap();
        let elsewhere = open(&dir, "elsewhere.sqlite3");
        add(&elsewhere, "old account's task");
        let app = Arc::new(open(&dir, "a.sqlite3"));
        let transport = AccountSwitchingTransport {
            app: Arc::clone(&app),
            inner: InMemoryTransport::new(),
        };
        transport
            .inner
            .push(
                "tok",
                elsewhere.peer_id(),
                elsewhere.export_for_push().unwrap(),
            )
            .unwrap();
        let engine = SyncEngine::new(Arc::new(transport), unused_auth());

        let result = engine.sync_once(&app, &session_for("a"));

        assert!(matches!(result, Err(SyncError::Auth(_))));
        assert!(titles(&app).is_empty());
        assert_eq!(app.last_pulled_seq().unwrap(), None);
    }

    #[test]
    fn a_round_whose_account_changes_before_the_push_pushes_nothing() {
        let dir = TempDir::new().unwrap();
        let app = Arc::new(open(&dir, "a.sqlite3"));
        let transport = Arc::new(AccountSwitchingTransport {
            app: Arc::clone(&app),
            inner: InMemoryTransport::new(),
        });
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth());

        // Empty log, so the loop never runs -- the check before the push
        // is the one that has to catch it.
        let result = engine.sync_once(&app, &session_for("a"));

        assert!(matches!(result, Err(SyncError::Auth(_))));
        assert!(transport.inner.log_rows().is_empty());
    }

    /// Pushes one row per title from `writer`, each its own Loro update --
    /// the shape a long-lived account's log has.
    fn push_one_row_per_task(transport: &InMemoryTransport, writer: &App, titles: &[String]) {
        for title in titles {
            add(writer, title);
            transport
                .push("tok", writer.peer_id(), writer.export_for_push().unwrap())
                .unwrap();
            writer.mark_pushed().unwrap();
        }
    }

    fn numbered(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix} {i}")).collect()
    }

    fn sorted_titles(app: &App) -> Vec<String> {
        let mut titles = titles(app);
        titles.sort();
        titles
    }

    #[test]
    fn a_fresh_device_rebuilds_from_the_snapshot_plus_the_tail() {
        let dir = TempDir::new().unwrap();
        let writer = open(&dir, "writer.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        push_one_row_per_task(&transport, &writer, &numbered("old", 3));
        let as_of = transport.log_rows().last().unwrap().seq;
        transport
            .compact("tok", as_of, writer.export_for_compaction().unwrap())
            .unwrap();
        push_one_row_per_task(&transport, &writer, &numbered("new", 2));

        let fresh = open(&dir, "fresh.sqlite3");
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth());
        let outcome = engine.sync_once(&fresh, &fresh_session("tok")).unwrap();

        assert_eq!(outcome.pulled, 3); // the snapshot plus two rows
        assert_eq!(sorted_titles(&fresh), sorted_titles(&writer));
        assert_eq!(
            fresh.last_pulled_seq().unwrap(),
            Some(transport.log_rows()[1].seq)
        );
    }

    #[test]
    fn a_device_whose_cursor_was_compacted_away_still_gets_every_change() {
        let dir = TempDir::new().unwrap();
        let writer = open(&dir, "writer.sqlite3");
        let lagging = open(&dir, "lagging.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth());
        push_one_row_per_task(&transport, &writer, &numbered("seen", 1));
        engine.sync_once(&lagging, &fresh_session("tok")).unwrap();

        // While `lagging` is away, more rows land and get compacted --
        // including the ones right after its cursor.
        push_one_row_per_task(&transport, &writer, &numbered("missed", 3));
        let as_of = transport.log_rows().last().unwrap().seq;
        transport
            .compact("tok", as_of, writer.export_for_compaction().unwrap())
            .unwrap();
        push_one_row_per_task(&transport, &writer, &numbered("after", 1));

        engine.sync_once(&lagging, &fresh_session("tok")).unwrap();

        assert_eq!(sorted_titles(&lagging), sorted_titles(&writer));
    }

    #[test]
    fn one_round_pulls_every_page() {
        let dir = TempDir::new().unwrap();
        let writer = open(&dir, "writer.sqlite3");
        let transport = Arc::new(InMemoryTransport::with_page_limit(2));
        push_one_row_per_task(&transport, &writer, &numbered("task", 5));

        let app = open(&dir, "a.sqlite3");
        let engine = SyncEngine::new(transport as Arc<dyn SyncTransport>, unused_auth());
        let outcome = engine.sync_once(&app, &fresh_session("tok")).unwrap();

        assert_eq!(outcome.pulled, 5);
        assert_eq!(sorted_titles(&app), sorted_titles(&writer));
    }

    #[test]
    fn a_round_that_finds_the_log_long_enough_compacts_it() {
        let dir = TempDir::new().unwrap();
        let writer = open(&dir, "writer.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        push_one_row_per_task(&transport, &writer, &numbered("task", 201));

        let app = open(&dir, "a.sqlite3");
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth());
        engine.sync_once(&app, &fresh_session("tok")).unwrap();

        // Folded up to the last row `app` pulled; only its own push, which
        // came after, is left.
        let snapshot = transport.snapshot().unwrap();
        assert_eq!(Some(snapshot.as_of_seq), app.last_pulled_seq().unwrap());
        assert_eq!(transport.log_rows().len(), 1);

        // ...and nothing was lost: a fresh device still gets all of it.
        let fresh = open(&dir, "fresh.sqlite3");
        engine.sync_once(&fresh, &fresh_session("tok")).unwrap();
        assert_eq!(sorted_titles(&fresh), sorted_titles(&writer));
    }

    #[test]
    fn a_log_at_the_threshold_is_left_alone() {
        let dir = TempDir::new().unwrap();
        let writer = open(&dir, "writer.sqlite3");
        let transport = Arc::new(InMemoryTransport::new());
        push_one_row_per_task(&transport, &writer, &numbered("task", 200));

        let app = open(&dir, "a.sqlite3");
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>, unused_auth());
        engine.sync_once(&app, &fresh_session("tok")).unwrap();

        assert_eq!(transport.snapshot(), None);
    }

    /// Reports a log long enough to compact, and fails the compaction.
    struct CompactFails;
    impl SyncTransport for CompactFails {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Ok(())
        }
        fn pull(&self, _: &str, _: Option<i64>) -> Result<crate::PullPage, SyncError> {
            Ok(crate::PullPage {
                // A row, so `compact` has a cursor to compact up to.
                rows: vec![crate::PulledUpdate {
                    seq: 0,
                    payload: open_scratch_export(),
                }],
                log_len: COMPACT_AFTER_ROWS + 1,
                ..Default::default()
            })
        }
        fn compact(&self, _: &str, _: i64, _: Vec<u8>) -> Result<bool, SyncError> {
            Err(SyncError::Transport("boom".to_string()))
        }
    }

    /// Real Loro bytes, since `import_from_pull` rejects anything else.
    fn open_scratch_export() -> Vec<u8> {
        let dir = TempDir::new().unwrap();
        open(&dir, "scratch.sqlite3").export_for_push().unwrap()
    }

    #[test]
    fn a_failed_compaction_does_not_fail_the_round() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let engine = SyncEngine::new(Arc::new(CompactFails), unused_auth());

        engine.sync_once(&app, &fresh_session("tok")).unwrap();

        assert!(app.last_synced_at().unwrap().is_some());
    }

    /// Claims there's more to pull but never hands any of it over.
    struct EndlessEmptyPages;
    impl SyncTransport for EndlessEmptyPages {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Ok(())
        }
        fn pull(&self, _: &str, _: Option<i64>) -> Result<crate::PullPage, SyncError> {
            Ok(crate::PullPage {
                has_more: true,
                ..Default::default()
            })
        }
        fn compact(&self, _: &str, _: i64, _: Vec<u8>) -> Result<bool, SyncError> {
            unreachable!("an empty log is never compacted")
        }
    }

    #[test]
    fn an_empty_page_ends_the_pull_even_if_it_claims_there_is_more() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let engine = SyncEngine::new(Arc::new(EndlessEmptyPages), unused_auth());

        assert_eq!(
            engine
                .sync_once(&app, &fresh_session("tok"))
                .unwrap()
                .pulled,
            0
        );
    }
}
