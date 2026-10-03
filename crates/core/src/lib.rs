#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod civil;
pub mod clock;
pub mod command;
mod doc;
mod due_parse;
pub mod snapshot;
mod store;

pub use clock::{Clock, IdSource, SystemClock, UuidSource};
#[cfg(any(test, feature = "testing"))]
pub use clock::{FixedClock, SeqIdSource};
pub use command::{Command, ListFilter, ViewFilter};
pub use doc::Doc;
pub use snapshot::{DueDetection, ListRow, Snapshot, TaskRow};
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
    /// Which list(s) `current()` reads — can be `ListFilter::All`, unlike
    /// `capture_list_id` below.
    current_list: ListFilter,
    /// The concrete list new captures land in — tracked separately from
    /// `current_list` because that can be "All", and a new task always
    /// needs one real destination list. Updated whenever `set_current_list`
    /// is given a concrete list, left alone when it's given "All". See
    /// `Store::load_capture_list`'s doc comment.
    capture_list_id: String,
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
        let current_list = match store.load_current_list()? {
            Some(id) => ListFilter::List(id),
            None => ListFilter::All,
        };
        let capture_list_id = store
            .load_capture_list()?
            .unwrap_or_else(|| doc::DEFAULT_LIST_ID.to_string());

        let shared = Arc::new(Shared {
            state: Mutex::new(AppState {
                doc,
                view: ViewFilter::default(),
                current_list,
                capture_list_id,
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
        let mut state = lock(&self.shared.state);
        let command = Self::resolve_capture_list(command, &state.capture_list_id);
        state
            .doc
            .apply(command, self.shared.clock.as_ref(), self.shared.ids.as_ref())?;
        drop(state);
        *lock(&self.shared.dirty_since) = Some(Instant::now());
        self.notify();
        Ok(())
    }

    /// Fills in `Command::Add`'s `list_id` with the sticky current capture
    /// list when the caller didn't specify one — the one piece of "which
    /// list is this app currently pointed at" business logic, kept here so
    /// every caller (CLI, Swift) gets it for free rather than reimplementing
    /// it per platform.
    fn resolve_capture_list(command: Command, capture_list_id: &str) -> Command {
        match command {
            Command::Add {
                title,
                after,
                due,
                list_id: None,
            } => Command::Add {
                title,
                after,
                due,
                list_id: Some(capture_list_id.to_string()),
            },
            other => other,
        }
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

    /// Switches which list(s) `current()` reads, persisting the choice (see
    /// `Store::load_current_list`) so it survives a restart. When `list` is
    /// a concrete list, also updates the sticky capture destination — see
    /// `capture_list_id`'s own doc comment for why "All" doesn't.
    pub fn set_current_list(&self, list: ListFilter) -> Result<(), CoreError> {
        self.shared.store.save_current_list(match &list {
            ListFilter::All => None,
            ListFilter::List(id) => Some(id.as_str()),
        })?;
        if let ListFilter::List(id) = &list {
            self.shared.store.save_capture_list(id)?;
        }

        let mut state = lock(&self.shared.state);
        if let ListFilter::List(id) = &list {
            state.capture_list_id = id.clone();
        }
        state.current_list = list;
        drop(state);

        self.notify();
        Ok(())
    }

    /// Sets the local UTC offset (seconds) used to compute "today" for both
    /// `current()`'s `due_label`/`overdue` fields and `detect_due`'s phrase
    /// resolution. Not a document mutation — see `Doc::set_local_offset_seconds`
    /// — but it does push a fresh snapshot, since existing rows' overdue
    /// status/labels can change even though nothing was actually edited
    /// (e.g. the device crossed into a new local day, or the user
    /// travelled). Call at launch and again whenever it might have changed:
    /// app foreground, and the system timezone-change notification.
    pub fn set_local_offset_seconds(&self, offset_seconds: i32) {
        lock(&self.shared.state)
            .doc
            .set_local_offset_seconds(offset_seconds);
        self.notify();
    }

    /// Detects a due-date phrase at the end of `text` (see [`due_parse`]),
    /// resolved against the current local day — the same "today" `current()`
    /// uses for `due_label`/`overdue`. A pure lookup: never touches the
    /// document, never commits, never bumps `revision`. `None` if `text`
    /// doesn't end in a recognized phrase.
    pub fn detect_due(&self, text: &str) -> Option<DueDetection> {
        let now = self.shared.clock.now();
        let offset = lock(&self.shared.state).doc.local_offset_seconds();
        let today = civil::day_number(now + i64::from(offset));
        let detection = due_parse::detect(text, today)?;
        let due = detection.day_number * civil::SECONDS_PER_DAY - i64::from(offset);
        Some(DueDetection {
            stripped_title: text[..detection.match_start].trim_end().to_string(),
            label: snapshot::due_label(due, now, offset),
            due,
        })
    }

    pub fn current(&self) -> Snapshot {
        let state = lock(&self.shared.state);
        let mut snapshot =
            state
                .doc
                .read(state.view, state.current_list.clone(), self.shared.clock.as_ref());
        snapshot.capture_list_id = state.capture_list_id.clone();
        snapshot
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
