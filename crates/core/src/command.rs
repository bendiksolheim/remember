#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `after: None` inserts at the top.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum ViewFilter {
    #[default]
    All,
    Active,
    Completed,
}
