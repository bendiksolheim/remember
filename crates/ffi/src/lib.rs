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
    Add {
        title: String,
        after: Option<String>,
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
    Undo,
    Redo,
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum View {
    All,
    Active,
    Completed,
}

#[derive(uniffi::Record)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub done: bool,
    pub due: Option<i64>,
    pub due_label: Option<String>,
    pub overdue: bool,
}

#[derive(uniffi::Record)]
pub struct Snapshot {
    pub rows: Vec<TaskRow>,
    pub view: View,
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
    // `Arc`, not a bare `CoreApp`, so `SyncClient::sync_now` can hand a
    // cheap clone to its background thread without `App` needing to
    // outlive it unsafely — see `SyncEngine::sync_now`'s own doc comment.
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

    pub fn current(&self) -> Snapshot {
        convert::snapshot_from_core(&self.inner.current())
    }

    pub fn flush(&self) -> Result<(), AppError> {
        self.inner.flush().map_err(convert::app_error_from_core)
    }
}

/// An authenticated device's credentials. Rust never persists this —
/// storing it (the platform Keychain) and handing it back on every call is
/// the caller's job, per the working agreement's Keychain exception.
#[derive(uniffi::Record)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
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
/// `sync_now` completing — the same `foreign`-only shape as
/// `SnapshotListener`.
#[uniffi::export(foreign)]
pub trait SyncStatusListener: Send + Sync {
    fn on_sync_complete(&self, outcome: SyncOutcome);
    fn on_sync_error(&self, error: SyncError);
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
        Arc::new(Self {
            auth: todo_sync::AuthClient::new(supabase_url, anon_key),
            engine: todo_sync::SyncEngine::new(transport),
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

    /// Runs a pull-then-push sync round on a background thread; `listener`
    /// is called back on completion or error. Never blocks the caller —
    /// see `SyncEngine::sync_now`'s doc comment for why this stays
    /// synchronous-looking rather than `async` at the FFI boundary.
    pub fn sync_now(&self, app: Arc<App>, session: Session, listener: Arc<dyn SyncStatusListener>) {
        let core_app = Arc::clone(&app.inner);
        self.engine
            .sync_now(core_app, session.access_token, move |result| match result {
                Ok(outcome) => listener.on_sync_complete(convert::sync_outcome_from_core(outcome)),
                Err(error) => listener.on_sync_error(convert::sync_error_from_core(error)),
            });
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
        coordinator.set_listener(move |result| match result {
            Ok(outcome) => listener.on_sync_complete(convert::sync_outcome_from_core(outcome)),
            Err(error) => listener.on_sync_error(convert::sync_error_from_core(error)),
        });
        *guard = Some(coordinator);
    }

    /// `None` pauses auto-sync (e.g. signed out) without stopping the
    /// background coordinator entirely. A no-op if `start_auto_sync` hasn't
    /// been called yet.
    pub fn set_sync_token(&self, session: Option<Session>) {
        let guard = self.auto_sync.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(coordinator) = guard.as_ref() {
            coordinator.set_token(session.map(|s| s.access_token));
        }
    }

    /// Requests an immediate auto-sync attempt — for platform lifecycle
    /// events (app foreground, the Mac waking from sleep) and right after
    /// signing in. A no-op if `start_auto_sync` hasn't been called yet.
    pub fn sync_soon(&self) {
        let guard = self.auto_sync.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(coordinator) = guard.as_ref() {
            coordinator.sync_soon();
        }
    }
}
