#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub mod clock;
pub mod command;
mod doc;
pub mod snapshot;
mod store;

pub use clock::{Clock, IdSource, SystemClock, UuidSource};
#[cfg(any(test, feature = "testing"))]
pub use clock::{FixedClock, SeqIdSource};
pub use command::{Command, ViewFilter};
pub use doc::Doc;
pub use snapshot::{Snapshot, TaskRow};
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

const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct AppState {
    doc: Doc,
    view: ViewFilter,
}

type Listener = Arc<dyn Fn(&Snapshot) + Send + Sync>;

struct Shared {
    state: Mutex<AppState>,
    store: Store,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdSource>,
    dirty_since: Mutex<Option<Instant>>,
    shutdown: AtomicBool,
    listeners: Mutex<Vec<Listener>>,
    peer_id: u64,
}

impl Shared {
    fn flush_now(&self) -> Result<(), CoreError> {
        let bytes = lock(&self.state).doc.export_snapshot()?;
        self.store.save_snapshot(&bytes, self.clock.now())
    }
}

/// Ties [`Doc`] and [`Store`] together: loads or creates the document on
/// open, debounces writes to disk, and flushes explicitly or on drop.
pub struct App {
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
}

impl App {
    pub fn open(db_path: &str) -> Result<Self, CoreError> {
        let store = Store::open(db_path)?;
        let peer_id = store.peer_id()?;
        let doc = match store.load_snapshot()? {
            Some(bytes) => Doc::load(peer_id, &bytes)?,
            None => Doc::new(peer_id)?,
        };

        let shared = Arc::new(Shared {
            state: Mutex::new(AppState {
                doc,
                view: ViewFilter::default(),
            }),
            store,
            clock: Arc::new(SystemClock),
            ids: Arc::new(UuidSource),
            dirty_since: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            listeners: Mutex::new(Vec::new()),
            peer_id,
        });

        let writer_shared = Arc::clone(&shared);
        let writer = thread::spawn(move || loop {
            thread::sleep(POLL_INTERVAL);
            if writer_shared.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let due = matches!(
                *lock(&writer_shared.dirty_since),
                Some(t) if t.elapsed() >= DEBOUNCE_WINDOW
            );
            if due {
                let _ = writer_shared.flush_now();
                *lock(&writer_shared.dirty_since) = None;
            }
        });

        Ok(Self {
            shared,
            writer: Some(writer),
        })
    }

    pub fn dispatch(&self, command: Command) -> Result<(), CoreError> {
        lock(&self.shared.state).doc.apply(
            command,
            self.shared.clock.as_ref(),
            self.shared.ids.as_ref(),
        )?;
        *lock(&self.shared.dirty_since) = Some(Instant::now());
        self.notify();
        Ok(())
    }

    /// Registers `listener` and fires it immediately with the current
    /// snapshot, so a caller never needs a separate initial-load path.
    /// Called again after every successful [`App::dispatch`].
    pub fn subscribe(&self, listener: impl Fn(&Snapshot) + Send + Sync + 'static) {
        let listener: Listener = Arc::new(listener);
        listener(&self.current());
        lock(&self.shared.listeners).push(listener);
    }

    fn notify(&self) {
        let snapshot = self.current();
        for listener in lock(&self.shared.listeners).iter() {
            listener(&snapshot);
        }
    }

    pub fn set_view(&self, view: ViewFilter) {
        lock(&self.shared.state).view = view;
        self.notify();
    }

    pub fn current(&self) -> Snapshot {
        let state = lock(&self.shared.state);
        state.doc.read(state.view, self.shared.clock.as_ref())
    }

    pub fn flush(&self) -> Result<(), CoreError> {
        self.shared.flush_now()?;
        *lock(&self.shared.dirty_since) = None;
        Ok(())
    }

    /// Stable for the life of this install — see [`Store::peer_id`].
    /// Exposed so a sync layer can tag pushed rows with their origin
    /// device.
    pub fn peer_id(&self) -> u64 {
        self.shared.peer_id
    }

    /// Whether there's anything worth pushing. A sync layer should check
    /// this before calling [`App::export_for_push`] — an export's byte
    /// length can't tell you "nothing changed" (see
    /// [`crate::Doc::has_changes_since`]), so this is the real check.
    pub fn has_unpushed_changes(&self) -> Result<bool, CoreError> {
        let since = self.shared.store.load_pushed_vv()?;
        lock(&self.shared.state)
            .doc
            .has_changes_since(since.as_deref())
    }

    /// Bytes to push for a sync round: everything since the last
    /// successful push (or the full history, on this device's first ever
    /// push). Does not itself advance the pushed cursor — call
    /// [`App::mark_pushed`] once the caller has confirmed the server
    /// accepted these bytes, so a failed push is safely retried as-is.
    pub fn export_for_push(&self) -> Result<Vec<u8>, CoreError> {
        let since = self.shared.store.load_pushed_vv()?;
        lock(&self.shared.state).doc.export_since(since.as_deref())
    }

    /// Records the current version vector as "already pushed". Call only
    /// after the sync transport confirms the bytes from
    /// [`App::export_for_push`] were accepted.
    pub fn mark_pushed(&self) -> Result<(), CoreError> {
        let vv = lock(&self.shared.state).doc.version_vector_bytes();
        self.shared.store.save_pushed_vv(&vv)
    }

    /// Merges bytes pulled from sync into this doc, then flushes and
    /// notifies subscribers the same way a local [`App::dispatch`] does —
    /// pulled changes should look identical to the UI as local edits.
    pub fn import_from_pull(&self, bytes: &[u8]) -> Result<(), CoreError> {
        lock(&self.shared.state).doc.import_updates(bytes)?;
        *lock(&self.shared.dirty_since) = Some(Instant::now());
        self.notify();
        Ok(())
    }

    /// The server log cursor as of the last successful pull. `None` means
    /// this device has never pulled — the caller should fetch from the
    /// start of the account's log.
    pub fn last_pulled_seq(&self) -> Result<Option<i64>, CoreError> {
        self.shared.store.load_pulled_seq()
    }

    /// Records `seq` as the last row this device has imported.
    pub fn mark_pulled(&self, seq: i64) -> Result<(), CoreError> {
        self.shared.store.save_pulled_seq(seq)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = self.flush();
        if let Some(handle) = self.writer.take() {
            let _ = handle.join();
        }
    }
}
