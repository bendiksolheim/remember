#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `after: None` inserts at the top. `list_id: None` means "whichever
    /// list is currently active" — resolved by `crate::App::dispatch`, not
    /// `Doc` itself, which falls back to the default list so callers that
    /// don't know about lists (the CLI, most existing tests) keep working
    /// unchanged. See `Doc`'s module doc comment for the default list's
    /// fixed id.
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
    /// Moves a task to a different list, landing at the top of its new
    /// list (see `Doc::apply`'s own doc comment on `SetList` for why).
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
    /// Deletes a list and every task in it, atomically — reversible with
    /// `Undo` like any other command, which is why there's no separate
    /// confirmation step at this layer.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum ViewFilter {
    #[default]
    All,
    Active,
    Completed,
}
