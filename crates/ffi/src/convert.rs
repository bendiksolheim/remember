//! All conversions between `remember-core` types and their UniFFI mirrors.
//! Plain functions, no decisions — covered at 100%.

use crate::{
    AppError, Command, DueDetection, DueState, ListColor, ListRow, Session, Snapshot, SyncError,
    SyncOutcome, TaskRow, View,
};

pub fn command_to_core(command: Command) -> remember_core::Command {
    match command {
        Command::Add {
            title,
            after,
            due,
            list_id,
        } => remember_core::Command::Add {
            title,
            after,
            due,
            list_id,
        },
        Command::SetTitle { id, title } => remember_core::Command::SetTitle { id, title },
        Command::SetNotes { id, notes } => remember_core::Command::SetNotes { id, notes },
        Command::SetDone { id, done } => remember_core::Command::SetDone { id, done },
        Command::SetDue { id, due } => remember_core::Command::SetDue { id, due },
        Command::Move { id, after } => remember_core::Command::Move { id, after },
        Command::Delete { id } => remember_core::Command::Delete { id },
        Command::SetList { id, list_id } => remember_core::Command::SetList { id, list_id },
        Command::AddList { name, after } => remember_core::Command::AddList { name, after },
        Command::RenameList { id, name } => remember_core::Command::RenameList { id, name },
        Command::DeleteList { id } => remember_core::Command::DeleteList { id },
        Command::MoveList { id, after } => remember_core::Command::MoveList { id, after },
        Command::Undo => remember_core::Command::Undo,
        Command::Redo => remember_core::Command::Redo,
    }
}

pub fn view_to_core(view: View) -> remember_core::ViewFilter {
    match view {
        View::All => remember_core::ViewFilter::All,
        View::Active => remember_core::ViewFilter::Active,
        View::Completed => remember_core::ViewFilter::Completed,
    }
}

pub fn view_from_core(view: remember_core::ViewFilter) -> View {
    match view {
        remember_core::ViewFilter::All => View::All,
        remember_core::ViewFilter::Active => View::Active,
        remember_core::ViewFilter::Completed => View::Completed,
    }
}

pub fn due_state_from_core(state: remember_core::DueState) -> DueState {
    match state {
        remember_core::DueState::None => DueState::None,
        remember_core::DueState::Later => DueState::Later,
        remember_core::DueState::Today => DueState::Today,
        remember_core::DueState::Overdue => DueState::Overdue,
    }
}

pub fn list_color_from_core(color: remember_core::ListColor) -> ListColor {
    match color {
        remember_core::ListColor::Blue => ListColor::Blue,
        remember_core::ListColor::Purple => ListColor::Purple,
        remember_core::ListColor::Pink => ListColor::Pink,
        remember_core::ListColor::Orange => ListColor::Orange,
        remember_core::ListColor::Teal => ListColor::Teal,
        remember_core::ListColor::Indigo => ListColor::Indigo,
        remember_core::ListColor::Mint => ListColor::Mint,
        remember_core::ListColor::Yellow => ListColor::Yellow,
        remember_core::ListColor::Cyan => ListColor::Cyan,
    }
}

pub fn list_row_from_core(row: &remember_core::ListRow) -> ListRow {
    ListRow {
        id: row.id.clone(),
        name: row.name.clone(),
        color: list_color_from_core(row.color),
        task_count: row.task_count,
    }
}

pub fn task_row_from_core(row: &remember_core::TaskRow) -> TaskRow {
    TaskRow {
        id: row.id.clone(),
        title: row.title.clone(),
        notes: row.notes.clone(),
        done: row.done,
        due: row.due,
        due_label: row.due_label.clone(),
        due_state: due_state_from_core(row.due_state),
        list_id: row.list_id.clone(),
        list_name: row.list_name.clone(),
    }
}

pub fn due_detection_from_core(detection: &remember_core::DueDetection) -> DueDetection {
    DueDetection {
        stripped_title: detection.stripped_title.clone(),
        label: detection.label.clone(),
        due: detection.due,
    }
}

pub fn snapshot_from_core(snapshot: &remember_core::Snapshot) -> Snapshot {
    Snapshot {
        rows: snapshot.rows.iter().map(task_row_from_core).collect(),
        view: view_from_core(snapshot.view),
        current_list: snapshot.current_list.clone(),
        lists: snapshot.lists.iter().map(list_row_from_core).collect(),
        active_count: snapshot.active_count,
        can_undo: snapshot.can_undo,
        can_redo: snapshot.can_redo,
        revision: snapshot.revision,
    }
}

pub fn app_error_from_core(error: remember_core::CoreError) -> AppError {
    match error {
        remember_core::CoreError::Storage(message) => AppError::Storage { message },
        remember_core::CoreError::Document(message) => AppError::Document { message },
        remember_core::CoreError::NotFound(message) => AppError::NotFound { message },
    }
}

pub fn session_from_sync(session: remember_sync::Session) -> Session {
    Session {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        user_id: session.user_id,
        email: session.email,
        expires_at: session.expires_at,
    }
}

pub fn session_to_sync(session: Session) -> remember_sync::Session {
    remember_sync::Session {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        user_id: session.user_id,
        email: session.email,
        expires_at: session.expires_at,
    }
}

pub fn sync_outcome_from_core(outcome: remember_sync::SyncOutcome) -> SyncOutcome {
    SyncOutcome {
        pulled: outcome.pulled as u64,
        pushed_bytes: outcome.pushed_bytes as u64,
    }
}

pub fn sync_error_from_core(error: remember_sync::SyncError) -> SyncError {
    match error {
        remember_sync::SyncError::Transport(message) => SyncError::Transport { message },
        remember_sync::SyncError::Auth(message) => SyncError::Auth { message },
        remember_sync::SyncError::Core(core_error) => SyncError::Core {
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
                after: None,
                due: Some(7),
                list_id: None,
            }),
            remember_core::Command::Add { title, after: None, due: Some(7), list_id: None } if title == "t"
        ));
        assert!(matches!(
            command_to_core(Command::Add {
                title: "t".to_string(),
                after: Some("x".to_string()),
                due: None,
                list_id: Some("l".to_string()),
            }),
            remember_core::Command::Add { after: Some(a), list_id: Some(l), .. } if a == "x" && l == "l"
        ));
        assert!(matches!(
            command_to_core(Command::SetTitle {
                id: "i".to_string(),
                title: "t".to_string()
            }),
            remember_core::Command::SetTitle { id, title } if id == "i" && title == "t"
        ));
        assert!(matches!(
            command_to_core(Command::SetNotes {
                id: "i".to_string(),
                notes: "n".to_string()
            }),
            remember_core::Command::SetNotes { id, notes } if id == "i" && notes == "n"
        ));
        assert!(matches!(
            command_to_core(Command::SetDone {
                id: "i".to_string(),
                done: true
            }),
            remember_core::Command::SetDone { id, done: true } if id == "i"
        ));
        assert!(matches!(
            command_to_core(Command::SetDue {
                id: "i".to_string(),
                due: Some(5)
            }),
            remember_core::Command::SetDue { due: Some(5), .. }
        ));
        assert!(matches!(
            command_to_core(Command::SetDue {
                id: "i".to_string(),
                due: None
            }),
            remember_core::Command::SetDue { due: None, .. }
        ));
        assert!(matches!(
            command_to_core(Command::Move {
                id: "i".to_string(),
                after: Some("a".to_string())
            }),
            remember_core::Command::Move { after: Some(a), .. } if a == "a"
        ));
        assert!(matches!(
            command_to_core(Command::Move {
                id: "i".to_string(),
                after: None
            }),
            remember_core::Command::Move { after: None, .. }
        ));
        assert!(matches!(
            command_to_core(Command::Delete {
                id: "i".to_string()
            }),
            remember_core::Command::Delete { id } if id == "i"
        ));
        assert!(matches!(
            command_to_core(Command::SetList {
                id: "i".to_string(),
                list_id: "l".to_string()
            }),
            remember_core::Command::SetList { id, list_id } if id == "i" && list_id == "l"
        ));
        assert!(matches!(
            command_to_core(Command::AddList {
                name: "n".to_string(),
                after: Some("a".to_string())
            }),
            remember_core::Command::AddList { name, after: Some(a) } if name == "n" && a == "a"
        ));
        assert!(matches!(
            command_to_core(Command::RenameList {
                id: "i".to_string(),
                name: "n".to_string()
            }),
            remember_core::Command::RenameList { id, name } if id == "i" && name == "n"
        ));
        assert!(matches!(
            command_to_core(Command::DeleteList {
                id: "i".to_string()
            }),
            remember_core::Command::DeleteList { id } if id == "i"
        ));
        assert!(matches!(
            command_to_core(Command::MoveList {
                id: "i".to_string(),
                after: None
            }),
            remember_core::Command::MoveList { after: None, .. }
        ));
        assert!(matches!(
            command_to_core(Command::Undo),
            remember_core::Command::Undo
        ));
        assert!(matches!(
            command_to_core(Command::Redo),
            remember_core::Command::Redo
        ));
    }

    #[test]
    fn view_round_trips_both_directions() {
        for (ffi, core) in [
            (View::All, remember_core::ViewFilter::All),
            (View::Active, remember_core::ViewFilter::Active),
            (View::Completed, remember_core::ViewFilter::Completed),
        ] {
            assert!(view_to_core(ffi) == core);
            assert!(view_from_core(core) == ffi);
        }
    }

    #[test]
    fn list_row_converts_all_fields() {
        let row = list_row_from_core(&remember_core::ListRow {
            id: "l".to_string(),
            name: "Work".to_string(),
            color: remember_core::ListColor::Teal,
            task_count: 4,
        });
        assert_eq!(row.id, "l");
        assert_eq!(row.name, "Work");
        assert_eq!(row.color, ListColor::Teal);
        assert_eq!(row.task_count, 4);
    }

    #[test]
    fn list_color_round_trips_every_variant() {
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Blue),
            ListColor::Blue
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Purple),
            ListColor::Purple
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Pink),
            ListColor::Pink
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Orange),
            ListColor::Orange
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Teal),
            ListColor::Teal
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Indigo),
            ListColor::Indigo
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Mint),
            ListColor::Mint
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Yellow),
            ListColor::Yellow
        );
        assert_eq!(
            list_color_from_core(remember_core::ListColor::Cyan),
            ListColor::Cyan
        );
    }

    fn sample_core_row(
        due: Option<i64>,
        due_label: Option<&str>,
        due_state: remember_core::DueState,
    ) -> remember_core::TaskRow {
        remember_core::TaskRow {
            id: "id".to_string(),
            title: "title".to_string(),
            notes: "notes".to_string(),
            done: true,
            due,
            due_label: due_label.map(str::to_string),
            due_state,
            list_id: "l".to_string(),
            list_name: "List".to_string(),
        }
    }

    #[test]
    fn task_row_converts_with_due_present() {
        let row = task_row_from_core(&sample_core_row(
            Some(42),
            Some("Today"),
            remember_core::DueState::Overdue,
        ));
        assert_eq!(row.id, "id");
        assert_eq!(row.title, "title");
        assert_eq!(row.notes, "notes");
        assert!(row.done);
        assert_eq!(row.due, Some(42));
        assert_eq!(row.due_label.as_deref(), Some("Today"));
        assert_eq!(row.due_state, DueState::Overdue);
        assert_eq!(row.list_id, "l");
        assert_eq!(row.list_name, "List");
    }

    #[test]
    fn task_row_converts_with_due_absent() {
        let row = task_row_from_core(&sample_core_row(None, None, remember_core::DueState::None));
        assert_eq!(row.due, None);
        assert_eq!(row.due_label, None);
        assert_eq!(row.due_state, DueState::None);
    }

    #[test]
    fn due_state_round_trips_every_variant() {
        assert_eq!(
            due_state_from_core(remember_core::DueState::None),
            DueState::None
        );
        assert_eq!(
            due_state_from_core(remember_core::DueState::Later),
            DueState::Later
        );
        assert_eq!(
            due_state_from_core(remember_core::DueState::Today),
            DueState::Today
        );
        assert_eq!(
            due_state_from_core(remember_core::DueState::Overdue),
            DueState::Overdue
        );
    }

    #[test]
    fn due_detection_converts_all_fields() {
        let detection = due_detection_from_core(&remember_core::DueDetection {
            stripped_title: "Buy milk".to_string(),
            label: "Tomorrow".to_string(),
            due: 86_400,
        });
        assert_eq!(detection.stripped_title, "Buy milk");
        assert_eq!(detection.label, "Tomorrow");
        assert_eq!(detection.due, 86_400);
    }

    #[test]
    fn snapshot_converts_rows_and_fields() {
        let core_snapshot = remember_core::Snapshot {
            rows: vec![sample_core_row(
                Some(1),
                Some("Today"),
                remember_core::DueState::Today,
            )],
            view: remember_core::ViewFilter::Active,
            current_list: "l".to_string(),
            lists: vec![remember_core::ListRow {
                id: "l".to_string(),
                name: "List".to_string(),
                color: remember_core::ListColor::Pink,
                task_count: 2,
            }],
            active_count: 3,
            can_undo: true,
            can_redo: false,
            revision: 7,
        };
        let snapshot = snapshot_from_core(&core_snapshot);
        assert_eq!(snapshot.rows.len(), 1);
        assert_eq!(snapshot.rows[0].id, "id");
        assert!(matches!(snapshot.view, View::Active));
        assert_eq!(snapshot.current_list, "l");
        assert_eq!(snapshot.lists.len(), 1);
        assert_eq!(snapshot.lists[0].id, "l");
        assert_eq!(snapshot.active_count, 3);
        assert!(snapshot.can_undo);
        assert!(!snapshot.can_redo);
        assert_eq!(snapshot.revision, 7);
    }

    #[test]
    fn snapshot_converts_empty_rows() {
        let core_snapshot = remember_core::Snapshot {
            rows: vec![],
            view: remember_core::ViewFilter::All,
            current_list: "default".to_string(),
            lists: vec![],
            active_count: 0,
            can_undo: false,
            can_redo: false,
            revision: 0,
        };
        let snapshot = snapshot_from_core(&core_snapshot);
        assert!(snapshot.rows.is_empty());
        assert_eq!(snapshot.current_list, "default");
        assert!(snapshot.lists.is_empty());
    }

    #[test]
    fn core_error_variants_map_preserving_message() {
        assert!(matches!(
            app_error_from_core(remember_core::CoreError::Storage("s".to_string())),
            AppError::Storage { message } if message == "s"
        ));
        assert!(matches!(
            app_error_from_core(remember_core::CoreError::Document("d".to_string())),
            AppError::Document { message } if message == "d"
        ));
        assert!(matches!(
            app_error_from_core(remember_core::CoreError::NotFound("n".to_string())),
            AppError::NotFound { message } if message == "n"
        ));
    }

    #[test]
    fn session_from_sync_converts_all_fields() {
        let session = session_from_sync(remember_sync::Session {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            user_id: "u".to_string(),
            email: Some("e@example.com".to_string()),
            expires_at: 42,
        });
        assert_eq!(session.access_token, "a");
        assert_eq!(session.refresh_token, "r");
        assert_eq!(session.user_id, "u");
        assert_eq!(session.email.as_deref(), Some("e@example.com"));
        assert_eq!(session.expires_at, 42);
    }

    #[test]
    fn session_to_sync_converts_all_fields() {
        let session = session_to_sync(Session {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            user_id: "u".to_string(),
            email: Some("e@example.com".to_string()),
            expires_at: 42,
        });
        assert_eq!(session.access_token, "a");
        assert_eq!(session.refresh_token, "r");
        assert_eq!(session.user_id, "u");
        assert_eq!(session.email.as_deref(), Some("e@example.com"));
        assert_eq!(session.expires_at, 42);
    }

    #[test]
    fn sync_outcome_converts_and_widens_to_u64() {
        let outcome = sync_outcome_from_core(remember_sync::SyncOutcome {
            pulled: 3,
            pushed_bytes: 128,
        });
        assert_eq!(outcome.pulled, 3);
        assert_eq!(outcome.pushed_bytes, 128);
    }

    #[test]
    fn sync_error_variants_map_preserving_message() {
        assert!(matches!(
            sync_error_from_core(remember_sync::SyncError::Transport("t".to_string())),
            SyncError::Transport { message } if message == "t"
        ));
        assert!(matches!(
            sync_error_from_core(remember_sync::SyncError::Auth("a".to_string())),
            SyncError::Auth { message } if message == "a"
        ));
        assert!(matches!(
            sync_error_from_core(remember_sync::SyncError::Core(remember_core::CoreError::NotFound(
                "n".to_string()
            ))),
            SyncError::Core { message } if message.contains('n')
        ));
    }
}
