//! UniFFI wrappers. Anything resembling a decision belongs in `todo-core`;
//! each method body here is a one-liner delegating to it via `convert.rs`.

mod convert;

use std::sync::{Arc, Mutex};

use todo_core::App as CoreApp;

uniffi::setup_scaffolding!();

/// Diagnostic probe. Kept permanently — the fastest way to confirm which
/// architecture slice is actually linked into a running app.
#[uniffi::export]
pub fn build_info() -> String {
    format!(
        "todo {} / {} / {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        std::env::consts::OS
    )
}

#[derive(uniffi::Enum)]
pub enum Command {
    /// `list_id: None` means "whichever list is currently active" — see
    /// `todo_core::Command::Add`'s own doc comment.
    Add {
        title: String,
        after: Option<String>,
        due: Option<i64>,
        list_id: Option<String>,
    },
    SetTitle {
        id: String,
        title: String,
    },
    SetNotes {
        id: String,
        notes: String,
    },
    SetDone {
        id: String,
        done: bool,
    },
    SetDue {
        id: String,
        due: Option<i64>,
    },
    Move {
        id: String,
        after: Option<String>,
    },
    Delete {
        id: String,
    },
    SetList {
        id: String,
        list_id: String,
    },
    AddList {
        name: String,
        after: Option<String>,
    },
    RenameList {
        id: String,
        name: String,
    },
    DeleteList {
        id: String,
    },
    MoveList {
        id: String,
        after: Option<String>,
    },
    Undo,
    Redo,
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum View {
    All,
    Active,
    Completed,
}

/// A task's urgency relative to "today". Mirrors `todo_core::DueState`.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DueState {
    #[default]
    None,
    Later,
    Today,
    Overdue,
}

/// One entry in the `lists` roster — see `todo_core::ListRow`.
#[derive(uniffi::Record, Clone)]
pub struct ListRow {
    pub id: String,
    pub name: String,
    pub color: ListColor,
    /// Every task in the list, done ones included -- what deleting it
    /// would delete.
    pub task_count: u32,
}

/// A list's identity color — an opaque palette slot, not a hex value.
/// Mirrors `todo_core::ListColor`.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListColor {
    #[default]
    Blue,
    Purple,
    Pink,
    Orange,
    Teal,
    Indigo,
    Mint,
    Yellow,
    Cyan,
}

#[derive(uniffi::Record)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub done: bool,
    pub due: Option<i64>,
    pub due_label: Option<String>,
    pub due_state: DueState,
    pub list_id: String,
    pub list_name: String,
}

/// Result of `App::detect_due` — see its own doc comment.
#[derive(uniffi::Record)]
pub struct DueDetection {
    pub stripped_title: String,
    pub label: String,
    pub due: i64,
}

#[derive(uniffi::Record)]
pub struct Snapshot {
    pub rows: Vec<TaskRow>,
    pub view: View,
    /// The list `current()` reflects, and the one new captures land in —
    /// always a concrete list id.
    pub current_list: String,
    pub lists: Vec<ListRow>,
    pub active_count: u32,
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AppError {
    #[error("{message}")]
    Storage { message: String },
    #[error("{message}")]
    Document { message: String },
    #[error("{message}")]
    NotFound { message: String },
}

/// Rust calls into Swift's implementation here; Swift never calls into a
/// Rust one, hence `foreign` rather than the bidirectional `rust, foreign`.
#[uniffi::export(foreign)]
pub trait SnapshotListener: Send + Sync {
    fn on_change(&self, snapshot: Snapshot);
}

#[derive(uniffi::Object)]
pub struct App {
    // `Arc`, not a bare `CoreApp`, so `SyncClient::start_auto_sync` can
    // hand a cheap clone to the coordinator's background thread without
    // `App` needing to outlive it unsafely.
    inner: Arc<CoreApp>,
}

#[uniffi::export]
impl App {
    #[uniffi::constructor]
    pub fn open(db_path: String) -> Result<Arc<Self>, AppError> {
        let inner = CoreApp::open(&db_path).map_err(convert::app_error_from_core)?;
        Ok(Arc::new(Self {
            inner: Arc::new(inner),
        }))
    }

    /// Fires `on_change` immediately with the current snapshot, so the
    /// caller has no separate initial-load path, then again after every
    /// successful `dispatch`.
    pub fn subscribe(&self, listener: Arc<dyn SnapshotListener>) {
        self.inner
            .subscribe(move |snapshot| listener.on_change(convert::snapshot_from_core(snapshot)));
    }

    pub fn dispatch(&self, command: Command) -> Result<(), AppError> {
        self.inner
            .dispatch(convert::command_to_core(command))
            .map_err(convert::app_error_from_core)
    }

    pub fn set_view(&self, view: View) {
        self.inner.set_view(convert::view_to_core(view));
    }

    /// Switches which list `current()` reads and new captures land in. See
    /// `todo_core::App::set_current_list`'s own doc comment.
    pub fn set_current_list(&self, list_id: String) -> Result<(), AppError> {
        self.inner
            .set_current_list(list_id)
            .map_err(convert::app_error_from_core)
    }

    pub fn current(&self) -> Snapshot {
        convert::snapshot_from_core(&self.inner.current())
    }

    /// Sets the device's local UTC offset (seconds), used to compute
    /// "today" for both `current()`'s due labels/overdue and `detect_due`.
    /// The caller must call this at launch and again whenever the offset
    /// might have changed — app foreground, and the system timezone-change
    /// notification — since `App` has no way to read the platform's
    /// timezone itself (Rust's own local-offset detection is unsound in a
    /// multithreaded process, which is exactly why this is a caller-driven
    /// setter rather than something computed on the Rust side).
    pub fn set_local_offset_seconds(&self, offset_seconds: i32) {
        self.inner.set_local_offset_seconds(offset_seconds);
    }

    /// Detects a due-date phrase ("today", "tomorrow", a weekday name, or
    /// "dec 25") at the end of `text`, for a live quick-add badge. `None`
    /// if `text` doesn't end in a recognized phrase.
    pub fn detect_due(&self, text: String) -> Option<DueDetection> {
        self.inner
            .detect_due(&text)
            .map(|d| convert::due_detection_from_core(&d))
    }

    pub fn flush(&self) -> Result<(), AppError> {
        self.inner.flush().map_err(convert::app_error_from_core)
    }

    /// When this device last finished a sync round without errors, in Unix
    /// seconds; `None` if never (for the signed-in account). Read it at
    /// launch and after each `SyncStatusListener::on_sync_complete`.
    pub fn last_synced_at(&self) -> Result<Option<i64>, AppError> {
        self.inner
            .last_synced_at()
            .map_err(convert::app_error_from_core)
    }
}

/// An authenticated device's credentials. Rust never persists this —
/// storing it (the platform Keychain) and handing it back on every call is
/// the caller's job, per the working agreement's Keychain exception.
/// Refreshing it, unlike storing it, happens on the Rust side (see
/// `SyncStatusListener::on_session_refreshed`) -- `expires_at` (Unix
/// seconds) is what lets `todo-sync` decide when that's due.
#[derive(uniffi::Record)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    /// For display only. `None` for sessions stored before this field
    /// existed, until their next refresh.
    pub email: Option<String>,
    pub expires_at: i64,
}

#[derive(uniffi::Record)]
pub struct SyncOutcome {
    pub pulled: u64,
    pub pushed_bytes: u64,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SyncError {
    #[error("{message}")]
    Transport { message: String },
    #[error("{message}")]
    Auth { message: String },
    #[error("{message}")]
    Core { message: String },
}

/// Rust calls into Swift's implementation here to report a background
/// auto-sync round completing — the same `foreign`-only shape as
/// `SnapshotListener`.
#[uniffi::export(foreign)]
pub trait SyncStatusListener: Send + Sync {
    fn on_sync_complete(&self, outcome: SyncOutcome);
    fn on_sync_error(&self, error: SyncError);
    /// Called whenever a sync attempt rotates the session's tokens --
    /// independent of `on_sync_complete`/`on_sync_error`, and always before
    /// whichever of those two follows. Supabase invalidates the old refresh
    /// token the instant a new one is issued, so the caller must persist
    /// this promptly (Keychain) even if the sync attempt that triggered the
    /// refresh then goes on to fail for an unrelated reason -- otherwise it's
    /// left holding a refresh token that no longer works.
    fn on_session_refreshed(&self, session: Session);
}

#[derive(uniffi::Object)]
pub struct SyncClient {
    auth: todo_sync::AuthClient,
    engine: todo_sync::SyncEngine,
    // Created lazily by `start_auto_sync` (needs an `App` to subscribe to,
    // which isn't available yet at construction) and shared by every
    // subsequent call — `set_sync_token`/`sync_soon` are no-ops before it
    // exists, matching "sync is opt-in, nothing runs until asked."
    auto_sync: Mutex<Option<Arc<todo_sync::AutoSyncCoordinator>>>,
}

#[uniffi::export]
impl SyncClient {
    #[uniffi::constructor]
    pub fn new(supabase_url: String, anon_key: String) -> Arc<Self> {
        let transport: Arc<dyn todo_sync::SyncTransport> = Arc::new(todo_sync::HttpTransport::new(
            supabase_url.clone(),
            anon_key.clone(),
        ));
        let auth = todo_sync::AuthClient::new(supabase_url, anon_key);
        Arc::new(Self {
            auth: auth.clone(),
            engine: todo_sync::SyncEngine::new(transport, auth),
            auto_sync: Mutex::new(None),
        })
    }

    pub fn sign_up(&self, email: String, password: String) -> Result<Session, SyncError> {
        self.auth
            .sign_up(&email, &password)
            .map(convert::session_from_sync)
            .map_err(convert::sync_error_from_core)
    }

    pub fn sign_in(&self, email: String, password: String) -> Result<Session, SyncError> {
        self.auth
            .sign_in(&email, &password)
            .map(convert::session_from_sync)
            .map_err(convert::sync_error_from_core)
    }

    /// Starts the background coordinator that pushes shortly after local
    /// edits settle and falls back to a periodic pull otherwise — see
    /// `AutoSyncCoordinator`'s own doc comment for the full trigger list.
    /// Idempotent: a second call is a no-op, since `App` (and therefore
    /// what to subscribe to) doesn't change for the lifetime of a
    /// `SyncClient`.
    pub fn start_auto_sync(&self, app: Arc<App>, listener: Arc<dyn SyncStatusListener>) {
        let mut guard = self.auto_sync.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_some() {
            return;
        }
        let coordinator =
            todo_sync::AutoSyncCoordinator::new(Arc::clone(&app.inner), self.engine.clone());
        let result_listener = Arc::clone(&listener);
        coordinator.set_listener(move |result| match result {
            Ok(outcome) => {
                result_listener.on_sync_complete(convert::sync_outcome_from_core(outcome))
            }
            Err(error) => result_listener.on_sync_error(convert::sync_error_from_core(error)),
        });
        coordinator.set_session_listener(move |session| {
            listener.on_session_refreshed(convert::session_from_sync(session));
        });
        *guard = Some(coordinator);
    }

    /// `None` pauses auto-sync (e.g. signed out) without stopping the
    /// background coordinator entirely. A no-op if `start_auto_sync` hasn't
    /// been called yet.
    pub fn set_sync_token(&self, session: Option<Session>) {
        let guard = self.auto_sync.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(coordinator) = guard.as_ref() {
            coordinator.set_session(session.map(convert::session_to_sync));
        }
    }

    /// Signs this device out: pauses auto-sync, and revokes the session at
    /// Supabase in the background — this device's only, so the user's
    /// other devices stay signed in. `session` is the caller's persisted
    /// copy, used if auto-sync has none. Best effort and never blocks:
    /// offline, the revocation is simply skipped. See `todo_sync::sign_out`.
    pub fn sign_out(&self, session: Option<Session>) {
        let guard = self.auto_sync.lock().unwrap_or_else(|p| p.into_inner());
        todo_sync::sign_out(
            &self.auth,
            guard.as_deref(),
            session.map(convert::session_to_sync),
        );
    }

    /// Requests an immediate auto-sync attempt — for platform lifecycle
    /// events (app foreground, the Mac waking from sleep), right after
    /// signing in, and "Sync Now", which has no round of its own so the
    /// session is only ever refreshed in one place. The result arrives
    /// through `start_auto_sync`'s listener. A no-op if `start_auto_sync`
    /// hasn't been called yet.
    pub fn sync_soon(&self) {
        let guard = self.auto_sync.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(coordinator) = guard.as_ref() {
            coordinator.sync_soon();
        }
    }
}
