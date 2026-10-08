use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::clock::{fresh_peer_id, Clock, IdSource, SystemClock, UuidSource};
use crate::command::{Command, ViewFilter};
use crate::doc::Doc;
use crate::session::Session;
use crate::snapshot::{DueDetection, Snapshot};
use crate::store::Store;
use crate::CoreError;

const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

type Listener = Arc<dyn Fn(&Snapshot) + Send + Sync>;

struct Shared {
    state: Mutex<Session>,
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
        let bytes = state.doc().export_snapshot()?;
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
        let current_list = store.load_current_list()?;

        let shared = Arc::new(Shared {
            state: Mutex::new(Session::new(doc, current_list)),
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
        let moved = state.dispatch(
            command,
            self.shared.clock.as_ref(),
            self.shared.ids.as_ref(),
        )?;
        if moved {
            self.shared.store.save_current_list(state.current_list())?;
        }
        drop(state);
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
        lock(&self.shared.state).set_view(view);
        self.notify();
    }

    /// Switches which list `current()` reads and new captures land in,
    /// persisting the choice (see `Store::load_current_list`) so it
    /// survives a restart.
    pub fn set_current_list(&self, list_id: String) -> Result<(), CoreError> {
        self.shared.store.save_current_list(&list_id)?;
        lock(&self.shared.state).set_current_list(list_id);
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
        lock(&self.shared.state).set_local_offset_seconds(offset_seconds);
        self.notify();
    }

    /// See [`Session::detect_due`].
    pub fn detect_due(&self, text: &str) -> Option<DueDetection> {
        lock(&self.shared.state).detect_due(text, self.shared.clock.as_ref())
    }

    pub fn current(&self) -> Snapshot {
        lock(&self.shared.state).current(self.shared.clock.as_ref())
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
    ///   everything, so tasks made before ever signing in get uploaded.
    /// - Same account: a no-op.
    /// - A different account: resets local state — empty document, both
    ///   sync cursors, the last sync time and the current list forgotten — so one account's
    ///   tasks never leak into, or get uploaded to, another's. The previous
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

        let peer_id = fresh_peer_id();
        let doc = Doc::new(peer_id)?;
        self.shared.store.reset_for_account(
            user_id,
            peer_id,
            &doc.export_snapshot()?,
            self.shared.clock.now(),
        )?;
        state.reset(doc);
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
            .doc()
            .has_changes_since(since.as_deref())
    }

    /// Bytes to push for a sync round: everything since the last
    /// successful push (or the full history, on this device's first ever
    /// push). Does not itself advance the pushed cursor — call
    /// [`App::mark_pushed`] once the caller has confirmed the server
    /// accepted these bytes, so a failed push is safely retried as-is.
    pub fn export_for_push(&self) -> Result<Vec<u8>, CoreError> {
        let since = self.shared.store.load_pushed_vv()?;
        lock(&self.shared.state)
            .doc()
            .export_since(since.as_deref())
    }

    /// The full document — current state plus its whole history — for the
    /// server to keep in place of the log rows it subsumes. Importing it
    /// (via [`App::import_from_pull`]) is equivalent to importing every
    /// update it contains, so a device that already has some of that
    /// history, or edits of its own the snapshot lacks, just merges it.
    pub fn export_for_compaction(&self) -> Result<Vec<u8>, CoreError> {
        lock(&self.shared.state).doc().export_snapshot()
    }

    /// Records the current version vector as "already pushed". Call only
    /// after the sync transport confirms the bytes from
    /// [`App::export_for_push`] were accepted.
    pub fn mark_pushed(&self) -> Result<(), CoreError> {
        let vv = lock(&self.shared.state).doc().version_vector_bytes();
        self.shared.store.save_pushed_vv(&vv)
    }

    /// Merges bytes pulled from sync into this doc, then flushes and
    /// notifies subscribers the same way a local [`App::dispatch`] does —
    /// pulled changes should look identical to the UI as local edits.
    pub fn import_from_pull(&self, bytes: &[u8]) -> Result<(), CoreError> {
        lock(&self.shared.state).doc_mut().import_updates(bytes)?;
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

    /// Records "now" as the end of a sync round that finished without
    /// errors. A sync layer calls this once a whole round (pull and push)
    /// has succeeded.
    pub fn mark_synced(&self) -> Result<(), CoreError> {
        self.shared
            .store
            .save_last_synced_at(self.shared.clock.now())
    }

    /// When [`App::mark_synced`] last ran, in Unix seconds. `None` if it
    /// never has, or not since a different account took over (see
    /// [`App::bind_sync_account`]). Persisted, so it survives a relaunch.
    pub fn last_synced_at(&self) -> Result<Option<i64>, CoreError> {
        self.shared.store.load_last_synced_at()
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn lock_recovers_a_mutex_poisoned_by_a_panicking_holder() {
        let mutex = Mutex::new(7);
        let _ = std::panic::catch_unwind(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poisons the mutex");
        });
        assert!(mutex.is_poisoned());

        assert_eq!(*lock(&mutex), 7);
    }
}
