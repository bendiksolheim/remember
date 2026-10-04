#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
pub use command::{Command, ViewFilter};
pub use doc::Doc;
pub use snapshot::{DueDetection, DueState, ListColor, ListRow, Snapshot, TaskRow};
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
    /// The list `current()` reads, and the one new captures land in —
    /// always a concrete list id.
    current_list: String,
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
    // Atomic, not a plain `u64`: `bind_sync_account` replaces it when a
    // different account takes over this install.
    peer_id: AtomicU64,
}

impl Shared {
    fn flush_now(&self) -> Result<(), CoreError> {
        // The state lock is held across the save, not just the export:
        // otherwise a `bind_sync_account` reset could land in between, and
        // this would write the previous account's document over the fresh
        // one the reset just stored.
        let state = lock(&self.state);
        let bytes = state.doc.export_snapshot()?;
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
        let current_list = store
            .load_current_list()?
            .unwrap_or_else(|| doc::DEFAULT_LIST_ID.to_string());

        let shared = Arc::new(Shared {
            state: Mutex::new(AppState {
                doc,
                view: ViewFilter::default(),
                current_list,
            }),
            store,
            clock: Arc::new(SystemClock),
            ids: Arc::new(UuidSource),
            dirty_since: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            listeners: Mutex::new(Vec::new()),
            peer_id: AtomicU64::new(peer_id),
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
        let command = Self::resolve_capture_list(command, &state.current_list);
        let deleted_list_id = match &command {
            Command::DeleteList { id } => Some(id.clone()),
            _ => None,
        };
        state.doc.apply(
            command,
            self.shared.clock.as_ref(),
            self.shared.ids.as_ref(),
        )?;
        if let Some(deleted_id) = deleted_list_id {
            self.repair_list_pointers(&mut state, &deleted_id)?;
        }
        drop(state);
        *lock(&self.shared.dirty_since) = Some(Instant::now());
        self.notify();
        Ok(())
    }

    /// Re-points `current_list` when a successful `DeleteList` just removed
    /// the list it was pointing at — otherwise the next capture would fail
    /// with `NotFound` (or the view would keep filtering on a list that no
    /// longer exists) until the user manually picked a new one. Falls back
    /// to the first list in sidebar order; `apply_delete_list`'s
    /// last-remaining-list guard means there's always at least one left.
    fn repair_list_pointers(
        &self,
        state: &mut AppState,
        deleted_id: &str,
    ) -> Result<(), CoreError> {
        if state.current_list != deleted_id {
            return Ok(());
        }

        let snapshot = state
            .doc
            .read(ViewFilter::All, deleted_id, self.shared.clock.as_ref());
        let fallback_id = snapshot
            .lists
            .first()
            .map(|list| list.id.clone())
            .ok_or_else(|| CoreError::Document("no lists remain after delete".to_string()))?;

        self.shared.store.save_current_list(&fallback_id)?;
        state.current_list = fallback_id;
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

    /// Switches which list `current()` reads and new captures land in,
    /// persisting the choice (see `Store::load_current_list`) so it
    /// survives a restart.
    pub fn set_current_list(&self, list_id: String) -> Result<(), CoreError> {
        self.shared.store.save_current_list(&list_id)?;
        lock(&self.shared.state).current_list = list_id;
        self.notify();
        Ok(())
    }

    /// Sets the local UTC offset (seconds) used to compute "today" for both
    /// `current()`'s `due_label`/`due_state` fields and `detect_due`'s phrase
    /// resolution. Not a document mutation — see `Doc::set_local_offset_seconds`
    /// — but it does push a fresh snapshot, since existing rows' due
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
    /// uses for `due_label`/`due_state`. A pure lookup: never touches the
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
        state
            .doc
            .read(state.view, &state.current_list, self.shared.clock.as_ref())
    }

    pub fn flush(&self) -> Result<(), CoreError> {
        self.shared.flush_now()?;
        *lock(&self.shared.dirty_since) = None;
        Ok(())
    }

    /// Stable for the life of this install — see [`Store::peer_id`] —
    /// until a different sync account takes it over (see
    /// [`App::bind_sync_account`]). Exposed so a sync layer can tag pushed
    /// rows with their origin device.
    pub fn peer_id(&self) -> u64 {
        self.shared.peer_id.load(Ordering::SeqCst)
    }

    /// Ties this install's local data and sync cursors to `user_id`. A sync
    /// layer must call this before every sync round, with the account it is
    /// about to sync as:
    ///
    /// - No account bound yet (first sign-in): binds `user_id` and keeps
    ///   everything, so todos made before ever signing in get uploaded.
    /// - Same account: a no-op.
    /// - A different account: resets local state — empty document, both
    ///   sync cursors and the current list forgotten — so one account's
    ///   todos never leak into, or get uploaded to, another's. The previous
    ///   account's synced data stays on the server; its unpushed edits on
    ///   this device are lost. Also picks a fresh peer id: the new account
    ///   may already hold ops this device made under its old peer id (it
    ///   synced here before), and a fresh document reusing that id would
    ///   mint conflicting op ids.
    ///
    /// Returns whether a reset happened.
    pub fn bind_sync_account(&self, user_id: &str) -> Result<bool, CoreError> {
        // Held throughout, so no edit, export or flush can interleave with
        // the swap (see `Shared::flush_now`, which holds it too).
        let mut state = lock(&self.shared.state);
        match self.shared.store.load_sync_account()? {
            Some(bound) if bound == user_id => return Ok(false),
            None => {
                self.shared.store.save_sync_account(user_id)?;
                return Ok(false);
            }
            Some(_) => {}
        }

        let peer_id = store::fresh_peer_id();
        let mut doc = Doc::new(peer_id)?;
        doc.set_local_offset_seconds(state.doc.local_offset_seconds());
        self.shared.store.reset_for_account(
            user_id,
            peer_id,
            &doc.export_snapshot()?,
            self.shared.clock.now(),
        )?;
        state.doc = doc;
        state.current_list = doc::DEFAULT_LIST_ID.to_string();
        self.shared.peer_id.store(peer_id, Ordering::SeqCst);
        drop(state);

        // Already on disk, so nothing pending for the writer thread.
        *lock(&self.shared.dirty_since) = None;
        self.notify();
        Ok(true)
    }

    /// The account [`App::bind_sync_account`] last bound, `None` if none
    /// ever was. Lets a sync round confirm it is still syncing the account
    /// it started with before touching local state.
    pub fn sync_account(&self) -> Result<Option<String>, CoreError> {
        self.shared.store.load_sync_account()
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
