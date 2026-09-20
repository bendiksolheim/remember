//! Orchestration: pull-then-merge-then-push, plus running that cycle on a
//! background thread. This is the one place that decides *when* to call
//! `App`'s sync primitives — `todo-ffi` must only delegate here, per the
//! FFI crate's own convention that decisions belong below it.

use std::sync::Arc;

use todo_core::App;

use crate::transport::SyncTransport;
use crate::SyncError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncOutcome {
    pub pulled: usize,
    pub pushed_bytes: usize,
}

#[derive(Clone)]
pub struct SyncEngine {
    transport: Arc<dyn SyncTransport>,
}

impl SyncEngine {
    pub fn new(transport: Arc<dyn SyncTransport>) -> Self {
        Self { transport }
    }

    /// One full sync round: pull first, so this device merges what other
    /// devices already contributed before contributing its own changes —
    /// this ordering doesn't affect correctness (Loro's merge is
    /// commutative either way) but means a device's own push always
    /// reflects the latest merged state. Synchronous; see [`Self::sync_now`]
    /// to run this off the calling thread.
    pub fn sync_once(&self, app: &App, token: &str) -> Result<SyncOutcome, SyncError> {
        let mut pulled = 0;
        let since = app.last_pulled_seq()?;
        for update in self.transport.pull(token, since)? {
            app.import_from_pull(&update.payload)?;
            app.mark_pulled(update.seq)?;
            pulled += 1;
        }

        let pushed_bytes = if app.has_unpushed_changes()? {
            let bytes = app.export_for_push()?;
            self.transport.push(token, app.peer_id(), bytes.clone())?;
            app.mark_pushed()?;
            bytes.len()
        } else {
            0
        };

        Ok(SyncOutcome {
            pulled,
            pushed_bytes,
        })
    }

    /// Runs [`Self::sync_once`] on a background thread and calls
    /// `on_complete` with the result — never panics the caller's thread,
    /// errors are delivered through the callback like everything else.
    /// `app` is an `Arc` so the FFI layer can hand over the same instance
    /// it holds elsewhere without the sync thread needing to outlive it
    /// unsafely.
    pub fn sync_now(
        &self,
        app: Arc<App>,
        token: String,
        on_complete: impl FnOnce(Result<SyncOutcome, SyncError>) + Send + 'static,
    ) {
        let transport = Arc::clone(&self.transport);
        std::thread::spawn(move || {
            let engine = SyncEngine { transport };
            let result = engine.sync_once(&app, &token);
            on_complete(result);
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::mpsc;

    use tempfile::TempDir;
    use todo_core::Command;

    use super::*;
    use crate::transport::InMemoryTransport;

    fn open(dir: &TempDir, name: &str) -> App {
        App::open(dir.path().join(name).to_str().unwrap()).unwrap()
    }

    fn add(app: &App, title: &str) {
        app.dispatch(Command::Add {
            title: title.to_string(),
            after: None,
        })
        .unwrap();
    }

    #[test]
    fn sync_once_with_nothing_local_and_nothing_remote_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        let transport: Arc<dyn SyncTransport> = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport);

        let outcome = engine.sync_once(&app, "tok").unwrap();
        assert_eq!(outcome, SyncOutcome::default());
    }

    #[test]
    fn sync_once_pushes_local_changes() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task");
        let transport = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport.clone() as Arc<dyn SyncTransport>);

        let outcome = engine.sync_once(&app, "tok").unwrap();
        assert!(outcome.pushed_bytes > 0);
        assert_eq!(transport.pull("tok", None).unwrap().len(), 1);
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
        let engine = SyncEngine::new(transport as Arc<dyn SyncTransport>);
        let outcome = engine.sync_once(&app, "tok").unwrap();

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
        let engine = SyncEngine::new(transport as Arc<dyn SyncTransport>);

        engine.sync_once(&device_a, "tok").unwrap();
        engine.sync_once(&device_b, "tok").unwrap();
        // device_b's pull above already saw device_a's push. device_a still
        // needs a second round to see device_b's.
        engine.sync_once(&device_a, "tok").unwrap();

        assert_eq!(device_a.current().rows.len(), 2);
        assert_eq!(device_b.current().rows.len(), 2);
    }

    #[test]
    fn sync_now_runs_in_the_background_and_reports_via_callback() {
        let dir = TempDir::new().unwrap();
        let app = Arc::new(open(&dir, "a.sqlite3"));
        add(&app, "task");
        let transport: Arc<dyn SyncTransport> = Arc::new(InMemoryTransport::new());
        let engine = SyncEngine::new(transport);

        let (tx, rx) = mpsc::channel();
        engine.sync_now(Arc::clone(&app), "tok".to_string(), move |result| {
            tx.send(result).unwrap();
        });

        let outcome = rx.recv().unwrap().unwrap();
        assert!(outcome.pushed_bytes > 0);
    }

    struct PullFails;
    impl SyncTransport for PullFails {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Err(SyncError::Transport(
                "push should not be reached".to_string(),
            ))
        }
        fn pull(&self, _: &str, _: Option<i64>) -> Result<Vec<crate::PulledUpdate>, SyncError> {
            Err(SyncError::Transport("boom".to_string()))
        }
    }

    struct PushFails;
    impl SyncTransport for PushFails {
        fn push(&self, _: &str, _: u64, _: Vec<u8>) -> Result<(), SyncError> {
            Err(SyncError::Transport("boom".to_string()))
        }
        fn pull(&self, _: &str, _: Option<i64>) -> Result<Vec<crate::PulledUpdate>, SyncError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn sync_now_delivers_errors_through_the_callback_not_a_panic() {
        let dir = TempDir::new().unwrap();
        let app = Arc::new(open(&dir, "a.sqlite3"));
        let engine = SyncEngine::new(Arc::new(PullFails));

        let (tx, rx) = mpsc::channel();
        engine.sync_now(app, "tok".to_string(), move |result| {
            tx.send(result).unwrap();
        });

        assert!(matches!(rx.recv().unwrap(), Err(SyncError::Transport(_))));
    }

    #[test]
    fn sync_once_surfaces_a_push_failure_after_a_successful_pull() {
        let dir = TempDir::new().unwrap();
        let app = open(&dir, "a.sqlite3");
        add(&app, "local task"); // ensures has_unpushed_changes() is true
        let engine = SyncEngine::new(Arc::new(PushFails));

        let err = engine.sync_once(&app, "tok").err().unwrap();
        assert!(matches!(err, SyncError::Transport(_)));
    }
}
