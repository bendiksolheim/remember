//! UniFFI wrappers. Anything resembling a decision belongs in `todo-core`;
//! each method body here is a one-liner delegating to it via `convert.rs`.

mod convert;

use std::sync::Arc;

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
    inner: CoreApp,
}

#[uniffi::export]
impl App {
    #[uniffi::constructor]
    pub fn open(db_path: String) -> Result<Arc<Self>, AppError> {
        let inner = CoreApp::open(&db_path).map_err(convert::app_error_from_core)?;
        Ok(Arc::new(Self { inner }))
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
