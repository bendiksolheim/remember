#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    /// `after: None` inserts at the top. `list_id: None` means "whichever
    /// list is currently active" — resolved by `crate::App::dispatch`, not
    /// `Doc` itself, which falls back to the default list so callers that
    /// don't know about lists (most existing tests) keep working
    /// unchanged. See `Doc`'s module doc comment for the default list's
    /// fixed id.
    Add {
        title: String,
        after: Option<String>,
        due: Option<i64>,
        list_id: Option<String>,
        /// Adds a last step to this task (or to this step's task); `after`
        /// and `list_id` are then ignored.
        #[serde(default)]
        parent_id: Option<String>,
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
    /// Makes `id` the last step of `parent_id`, or with `None` a top-level
    /// task right below its former task. Rejected across lists, under a
    /// step, or when `id` has steps.
    SetParent {
        id: String,
        parent_id: Option<String>,
    },
    /// Tab: makes `id` the last step of the nearest top-level task above it
    /// in the current view. `Indent`, `Outdent` and `Shift` are resolved by
    /// `crate::Session` against what's shown and never reach `Doc`.
    Indent {
        id: String,
    },
    /// ⇧Tab: a step becomes a top-level task right below its former task.
    Outdent {
        id: String,
    },
    /// Moves `id` by `delta` places among its peers in the current view,
    /// clamped: a step among its siblings, a task past whole tasks.
    Shift {
        id: String,
        delta: i32,
    },
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ViewFilter {
    #[default]
    All,
    Active,
    Completed,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The web build receives commands as JS objects in this shape.
    #[test]
    fn deserializes_internally_tagged_json() {
        let add: Command = serde_json::from_str(
            r#"{"type":"Add","title":"Milk","after":null,"due":5,"list_id":null}"#,
        )
        .expect("add");
        assert_eq!(
            add,
            Command::Add {
                title: "Milk".into(),
                after: None,
                due: Some(5),
                list_id: None,
                parent_id: None,
            }
        );
        let step: Command = serde_json::from_str(
            r#"{"type":"Add","title":"Milk","after":null,"due":null,"list_id":null,"parent_id":"p"}"#,
        )
        .expect("add step");
        assert!(matches!(step, Command::Add { parent_id: Some(p), .. } if p == "p"));
        let shift: Command =
            serde_json::from_str(r#"{"type":"Shift","id":"a","delta":-1}"#).expect("shift");
        assert_eq!(
            shift,
            Command::Shift {
                id: "a".into(),
                delta: -1
            }
        );
        let outdent: Command =
            serde_json::from_str(r#"{"type":"SetParent","id":"a","parent_id":null}"#)
                .expect("set parent");
        assert_eq!(
            outdent,
            Command::SetParent {
                id: "a".into(),
                parent_id: None
            }
        );
        let undo: Command = serde_json::from_str(r#"{"type":"Undo"}"#).expect("undo");
        assert_eq!(undo, Command::Undo);
        let view: ViewFilter = serde_json::from_str(r#""Completed""#).expect("view");
        assert_eq!(view, ViewFilter::Completed);
    }
}
