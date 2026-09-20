//! All conversions between `todo-core` types and their UniFFI mirrors.
//! Plain functions, no decisions — covered at 100%.

use crate::{AppError, Command, Session, Snapshot, SyncError, SyncOutcome, TaskRow, View};

pub fn command_to_core(command: Command) -> todo_core::Command {
    match command {
        Command::Add { title, after } => todo_core::Command::Add { title, after },
        Command::SetTitle { id, title } => todo_core::Command::SetTitle { id, title },
        Command::SetNotes { id, notes } => todo_core::Command::SetNotes { id, notes },
        Command::SetDone { id, done } => todo_core::Command::SetDone { id, done },
        Command::SetDue { id, due } => todo_core::Command::SetDue { id, due },
        Command::Move { id, after } => todo_core::Command::Move { id, after },
        Command::Delete { id } => todo_core::Command::Delete { id },
        Command::Undo => todo_core::Command::Undo,
        Command::Redo => todo_core::Command::Redo,
    }
}

pub fn view_to_core(view: View) -> todo_core::ViewFilter {
    match view {
        View::All => todo_core::ViewFilter::All,
        View::Active => todo_core::ViewFilter::Active,
        View::Completed => todo_core::ViewFilter::Completed,
    }
}

pub fn view_from_core(view: todo_core::ViewFilter) -> View {
    match view {
        todo_core::ViewFilter::All => View::All,
        todo_core::ViewFilter::Active => View::Active,
        todo_core::ViewFilter::Completed => View::Completed,
    }
}

pub fn task_row_from_core(row: &todo_core::TaskRow) -> TaskRow {
    TaskRow {
        id: row.id.clone(),
        title: row.title.clone(),
        notes: row.notes.clone(),
        done: row.done,
        due: row.due,
        due_label: row.due_label.clone(),
        overdue: row.overdue,
    }
}

pub fn snapshot_from_core(snapshot: &todo_core::Snapshot) -> Snapshot {
    Snapshot {
        rows: snapshot.rows.iter().map(task_row_from_core).collect(),
        view: view_from_core(snapshot.view),
        active_count: snapshot.active_count,
        can_undo: snapshot.can_undo,
        can_redo: snapshot.can_redo,
        revision: snapshot.revision,
    }
}

pub fn app_error_from_core(error: todo_core::CoreError) -> AppError {
    match error {
        todo_core::CoreError::Storage(message) => AppError::Storage { message },
        todo_core::CoreError::Document(message) => AppError::Document { message },
        todo_core::CoreError::NotFound(message) => AppError::NotFound { message },
    }
}

pub fn session_from_sync(session: todo_sync::Session) -> Session {
    Session {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        user_id: session.user_id,
    }
}

pub fn sync_outcome_from_core(outcome: todo_sync::SyncOutcome) -> SyncOutcome {
    SyncOutcome {
        pulled: outcome.pulled as u64,
        pushed_bytes: outcome.pushed_bytes as u64,
    }
}

pub fn sync_error_from_core(error: todo_sync::SyncError) -> SyncError {
    match error {
        todo_sync::SyncError::Transport(message) => SyncError::Transport { message },
        todo_sync::SyncError::Auth(message) => SyncError::Auth { message },
        todo_sync::SyncError::Core(core_error) => SyncError::Core {
            message: core_error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_variants_convert() {
        assert!(matches!(
            command_to_core(Command::Add {
                title: "t".to_string(),
                after: None
            }),
            todo_core::Command::Add { title, after: None } if title == "t"
        ));
        assert!(matches!(
            command_to_core(Command::Add {
                title: "t".to_string(),
                after: Some("x".to_string())
            }),
            todo_core::Command::Add { after: Some(a), .. } if a == "x"
        ));
        assert!(matches!(
            command_to_core(Command::SetTitle {
                id: "i".to_string(),
                title: "t".to_string()
            }),
            todo_core::Command::SetTitle { id, title } if id == "i" && title == "t"
        ));
        assert!(matches!(
            command_to_core(Command::SetNotes {
                id: "i".to_string(),
                notes: "n".to_string()
            }),
            todo_core::Command::SetNotes { id, notes } if id == "i" && notes == "n"
        ));
        assert!(matches!(
            command_to_core(Command::SetDone {
                id: "i".to_string(),
                done: true
            }),
            todo_core::Command::SetDone { id, done: true } if id == "i"
        ));
        assert!(matches!(
            command_to_core(Command::SetDue {
                id: "i".to_string(),
                due: Some(5)
            }),
            todo_core::Command::SetDue { due: Some(5), .. }
        ));
        assert!(matches!(
            command_to_core(Command::SetDue {
                id: "i".to_string(),
                due: None
            }),
            todo_core::Command::SetDue { due: None, .. }
        ));
        assert!(matches!(
            command_to_core(Command::Move {
                id: "i".to_string(),
                after: Some("a".to_string())
            }),
            todo_core::Command::Move { after: Some(a), .. } if a == "a"
        ));
        assert!(matches!(
            command_to_core(Command::Move {
                id: "i".to_string(),
                after: None
            }),
            todo_core::Command::Move { after: None, .. }
        ));
        assert!(matches!(
            command_to_core(Command::Delete {
                id: "i".to_string()
            }),
            todo_core::Command::Delete { id } if id == "i"
        ));
        assert!(matches!(
            command_to_core(Command::Undo),
            todo_core::Command::Undo
        ));
        assert!(matches!(
            command_to_core(Command::Redo),
            todo_core::Command::Redo
        ));
    }

    #[test]
    fn view_round_trips_both_directions() {
        for (ffi, core) in [
            (View::All, todo_core::ViewFilter::All),
            (View::Active, todo_core::ViewFilter::Active),
            (View::Completed, todo_core::ViewFilter::Completed),
        ] {
            assert!(view_to_core(ffi) == core);
            assert!(view_from_core(core) == ffi);
        }
    }

    fn sample_core_row(due: Option<i64>, due_label: Option<&str>) -> todo_core::TaskRow {
        todo_core::TaskRow {
            id: "id".to_string(),
            title: "title".to_string(),
            notes: "notes".to_string(),
            done: true,
            due,
            due_label: due_label.map(str::to_string),
            overdue: due.is_some(),
        }
    }

    #[test]
    fn task_row_converts_with_due_present() {
        let row = task_row_from_core(&sample_core_row(Some(42), Some("Today")));
        assert_eq!(row.id, "id");
        assert_eq!(row.title, "title");
        assert_eq!(row.notes, "notes");
        assert!(row.done);
        assert_eq!(row.due, Some(42));
        assert_eq!(row.due_label.as_deref(), Some("Today"));
        assert!(row.overdue);
    }

    #[test]
    fn task_row_converts_with_due_absent() {
        let row = task_row_from_core(&sample_core_row(None, None));
        assert_eq!(row.due, None);
        assert_eq!(row.due_label, None);
        assert!(!row.overdue);
    }

    #[test]
    fn snapshot_converts_rows_and_fields() {
        let core_snapshot = todo_core::Snapshot {
            rows: vec![sample_core_row(Some(1), Some("Today"))],
            view: todo_core::ViewFilter::Active,
            active_count: 3,
            can_undo: true,
            can_redo: false,
            revision: 7,
        };
        let snapshot = snapshot_from_core(&core_snapshot);
        assert_eq!(snapshot.rows.len(), 1);
        assert_eq!(snapshot.rows[0].id, "id");
        assert!(matches!(snapshot.view, View::Active));
        assert_eq!(snapshot.active_count, 3);
        assert!(snapshot.can_undo);
        assert!(!snapshot.can_redo);
        assert_eq!(snapshot.revision, 7);
    }

    #[test]
    fn snapshot_converts_empty_rows() {
        let core_snapshot = todo_core::Snapshot {
            rows: vec![],
            view: todo_core::ViewFilter::All,
            active_count: 0,
            can_undo: false,
            can_redo: false,
            revision: 0,
        };
        assert!(snapshot_from_core(&core_snapshot).rows.is_empty());
    }

    #[test]
    fn core_error_variants_map_preserving_message() {
        assert!(matches!(
            app_error_from_core(todo_core::CoreError::Storage("s".to_string())),
            AppError::Storage { message } if message == "s"
        ));
        assert!(matches!(
            app_error_from_core(todo_core::CoreError::Document("d".to_string())),
            AppError::Document { message } if message == "d"
        ));
        assert!(matches!(
            app_error_from_core(todo_core::CoreError::NotFound("n".to_string())),
            AppError::NotFound { message } if message == "n"
        ));
    }

    #[test]
    fn session_converts_all_fields() {
        let session = session_from_sync(todo_sync::Session {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            user_id: "u".to_string(),
        });
        assert_eq!(session.access_token, "a");
        assert_eq!(session.refresh_token, "r");
        assert_eq!(session.user_id, "u");
    }

    #[test]
    fn sync_outcome_converts_and_widens_to_u64() {
        let outcome = sync_outcome_from_core(todo_sync::SyncOutcome {
            pulled: 3,
            pushed_bytes: 128,
        });
        assert_eq!(outcome.pulled, 3);
        assert_eq!(outcome.pushed_bytes, 128);
    }

    #[test]
    fn sync_error_variants_map_preserving_message() {
        assert!(matches!(
            sync_error_from_core(todo_sync::SyncError::Transport("t".to_string())),
            SyncError::Transport { message } if message == "t"
        ));
        assert!(matches!(
            sync_error_from_core(todo_sync::SyncError::Auth("a".to_string())),
            SyncError::Auth { message } if message == "a"
        ));
        assert!(matches!(
            sync_error_from_core(todo_sync::SyncError::Core(todo_core::CoreError::NotFound(
                "n".to_string()
            ))),
            SyncError::Core { message } if message.contains('n')
        ));
    }
}
