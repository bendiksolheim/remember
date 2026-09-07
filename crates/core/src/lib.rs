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
