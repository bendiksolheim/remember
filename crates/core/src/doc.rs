//! The only module that may mention Loro. Document layout:
//!
//! - `tasks` — a `LoroMap` keyed by task UUID, each value a nested `LoroMap`
//!   with `title`, `notes`, `done`, `due`, `created_at`.
//! - `order` — a `LoroMovableList` of task UUID strings.
//!
//! No derived state (sort index, cached `overdue`, formatted date) is ever
//! stored in the document — all of that is computed in `read()`.

use std::cmp::Ordering;

use loro::{ExportMode, LoroDoc, LoroMap, LoroMovableList, LoroValue, UndoManager};

use crate::clock::{Clock, IdSource};
use crate::command::{Command, ViewFilter};
use crate::snapshot::{due_label, is_overdue, matches_filter, Snapshot, TaskRow};
use crate::CoreError;

pub struct Doc {
    doc: LoroDoc,
    undo: UndoManager,
    revision: u64,
}

fn doc_err<E: std::fmt::Display>(e: E) -> CoreError {
    CoreError::Document(e.to_string())
}

fn as_string(value: &LoroValue) -> Option<String> {
    match value {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn as_bool(value: &LoroValue) -> Option<bool> {
    match value {
        LoroValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn as_i64(value: &LoroValue) -> Option<i64> {
    match value {
        LoroValue::I64(n) => Some(*n),
        _ => None,
    }
}

fn order_ids(order: &LoroMovableList) -> Vec<String> {
    order.to_vec().iter().filter_map(as_string).collect()
}

impl Doc {
    pub fn new(peer_id: u64) -> Result<Self, CoreError> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(doc_err)?;
        // Default is 1000s: consecutive commits within the window merge into
        // one underlying Change, which breaks "one commit() per user action"
        // — undo would then revert several commands at once.
        doc.set_change_merge_interval(0);
        let undo = UndoManager::new(&doc);
        Ok(Self {
            doc,
            undo,
            revision: 0,
        })
    }

    pub fn load(peer_id: u64, snapshot: &[u8]) -> Result<Self, CoreError> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(doc_err)?;
        doc.set_change_merge_interval(0);
        doc.import(snapshot).map_err(doc_err)?;
        let undo = UndoManager::new(&doc);
        Ok(Self {
            doc,
            undo,
            revision: 0,
        })
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>, CoreError> {
        self.doc.export(ExportMode::snapshot()).map_err(doc_err)
    }

    /// Full-history update export. A minimal stand-in for the
    /// version-vector-scoped `export_updates_since` that Phase 11's sync
    /// seam calls for — the Phase 5 property tests need *a* merge pathway
    /// now; incremental scoping is a sync-efficiency concern, not a
    /// convergence one, so it's deferred to Phase 11.
    pub fn export_updates(&self) -> Result<Vec<u8>, CoreError> {
        self.doc.export(ExportMode::all_updates()).map_err(doc_err)
    }

    /// Merges updates from another peer into this doc — the same `import`
    /// primitive `load` uses, just against an already-populated document.
    pub fn import_updates(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        self.doc.import(bytes).map_err(doc_err)?;
        Ok(())
    }

    /// Number of tasks in the `tasks` map. Exposed only so property tests
    /// (outside this module) can verify structural consistency against
    /// [`Doc::order_len`] without reaching into Loro themselves.
    pub fn task_count(&self) -> usize {
        self.doc.get_map("tasks").len()
    }

    /// Number of ids in the `order` list. See [`Doc::task_count`].
    pub fn order_len(&self) -> usize {
        self.doc.get_movable_list("order").len()
    }

    pub fn apply(
        &mut self,
        cmd: Command,
        clock: &dyn Clock,
        ids: &dyn IdSource,
    ) -> Result<(), CoreError> {
        match cmd {
            Command::Add { title, after } => self.apply_add(title, after, clock, ids),
            Command::SetTitle { id, title } => self.apply_set_title(&id, title),
            Command::SetNotes { id, notes } => self.set_field(&id, "notes", notes.into()),
            Command::SetDone { id, done } => self.set_field(&id, "done", done.into()),
            Command::SetDue { id, due } => self.apply_set_due(&id, due),
            Command::Move { id, after } => self.apply_move(&id, after),
            Command::Delete { id } => self.apply_delete(&id),
            Command::Undo => {
                if self.undo.undo().map_err(doc_err)? {
                    self.revision += 1;
                }
                Ok(())
            }
            Command::Redo => {
                if self.undo.redo().map_err(doc_err)? {
                    self.revision += 1;
                }
                Ok(())
            }
        }
    }

    pub fn read(&self, view: ViewFilter, clock: &dyn Clock) -> Snapshot {
        let now = clock.now();
        let tasks = self.doc.get_map("tasks");
        let order = self.doc.get_movable_list("order");

        let mut rows = Vec::new();
        let mut active_count = 0u32;
        for id in order_ids(&order) {
            let Some(entry) = tasks.get(&id) else {
                continue; // defensive: order references a task `tasks` doesn't have
            };
            let LoroValue::Map(fields) = entry.get_deep_value() else {
                continue; // defensive: entry isn't the expected nested map
            };
            let Some(title) = fields.get("title").and_then(as_string) else {
                continue;
            };
            let notes = fields.get("notes").and_then(as_string).unwrap_or_default();
            let done = fields.get("done").and_then(as_bool).unwrap_or(false);
            let due = fields.get("due").and_then(as_i64);

            if !done {
                active_count += 1;
            }
            if !matches_filter(done, view) {
                continue;
            }

            rows.push(TaskRow {
                id,
                title,
                notes,
                done,
                due,
                due_label: due.map(|d| due_label(d, now)),
                overdue: due.is_some_and(|d| is_overdue(d, now)),
            });
        }

        Snapshot {
            rows,
            view,
            active_count,
            can_undo: self.undo.can_undo(),
            can_redo: self.undo.can_redo(),
            revision: self.revision,
        }
    }

    fn existing_task_map(&self, id: &str) -> Result<LoroMap, CoreError> {
        let tasks = self.doc.get_map("tasks");
        if tasks.get(id).is_none() {
            return Err(CoreError::NotFound(id.to_string()));
        }
        tasks.ensure_mergeable_map(id).map_err(doc_err)
    }

    fn apply_set_title(&mut self, id: &str, title: String) -> Result<(), CoreError> {
        // Check the id before the title: an unknown id must report NotFound
        // even when the title is also empty, not get masked by the no-op path.
        self.existing_task_map(id)?;
        let trimmed = title.trim();
        if trimmed.is_empty() {
            // Same reasoning as set_field's same-value guard: an empty title
            // must not commit or bump `revision`, or Undo would revert the
            // previous real change instead of being a true no-op.
            return Ok(());
        }
        self.set_field(id, "title", trimmed.into())
    }

    fn set_field(&mut self, id: &str, field: &str, value: LoroValue) -> Result<(), CoreError> {
        let task = self.existing_task_map(id)?;
        // Writing a value identical to the current one produces no new Loro
        // op (its LWW register dedupes same-value writes), so it must not
        // commit or bump `revision` either — otherwise `Undo` afterward has
        // nothing of its own to undo and reverts the *previous* real change.
        if task.get(field).map(|v| v.get_deep_value()).as_ref() == Some(&value) {
            return Ok(());
        }
        task.insert(field, value).map_err(doc_err)?;
        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn apply_set_due(&mut self, id: &str, due: Option<i64>) -> Result<(), CoreError> {
        let task = self.existing_task_map(id)?;
        let value: LoroValue = match due {
            Some(d) => d.into(),
            None => LoroValue::Null,
        };
        if task.get("due").map(|v| v.get_deep_value()).as_ref() == Some(&value) {
            return Ok(());
        }
        task.insert("due", value).map_err(doc_err)?;
        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn insert_position(
        &self,
        order: &LoroMovableList,
        after: Option<&str>,
    ) -> Result<usize, CoreError> {
        match after {
            None => Ok(0),
            Some(after_id) => {
                let ids = order_ids(order);
                let idx = ids
                    .iter()
                    .position(|x| x == after_id)
                    .ok_or_else(|| CoreError::NotFound(after_id.to_string()))?;
                Ok(idx + 1)
            }
        }
    }

    fn apply_add(
        &mut self,
        title: String,
        after: Option<String>,
        clock: &dyn Clock,
        ids: &dyn IdSource,
    ) -> Result<(), CoreError> {
        let order = self.doc.get_movable_list("order");
        let pos = self.insert_position(&order, after.as_deref())?;

        let trimmed = title.trim();
        if trimmed.is_empty() {
            // Mirrors apply_set_title: an empty title is a full no-op here,
            // not a task with a blank name — nothing is created, `revision`
            // doesn't move.
            return Ok(());
        }

        let id = ids.new_id();
        let tasks = self.doc.get_map("tasks");
        let task = tasks.ensure_mergeable_map(&id).map_err(doc_err)?;
        task.insert("title", trimmed).map_err(doc_err)?;
        task.insert("notes", "").map_err(doc_err)?;
        task.insert("done", false).map_err(doc_err)?;
        task.insert("due", LoroValue::Null).map_err(doc_err)?;
        task.insert("created_at", clock.now()).map_err(doc_err)?;

        order.insert(pos, id).map_err(doc_err)?;

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn apply_move(&mut self, id: &str, after: Option<String>) -> Result<(), CoreError> {
        let order = self.doc.get_movable_list("order");
        let ids = order_ids(&order);
        let from = ids
            .iter()
            .position(|x| x == id)
            .ok_or_else(|| CoreError::NotFound(id.to_string()))?;

        let to = match after.as_deref() {
            None => 0,
            Some(after_id) => {
                let after_idx = ids
                    .iter()
                    .position(|x| x == after_id)
                    .ok_or_else(|| CoreError::NotFound(after_id.to_string()))?;
                match after_idx.cmp(&from) {
                    Ordering::Less => after_idx + 1,
                    Ordering::Greater => after_idx,
                    Ordering::Equal => from,
                }
            }
        };

        if to == from {
            // Genuinely a no-op (e.g. already first, or already right after
            // the given id) — see the comment in `set_field` for why this
            // must not commit or bump `revision`.
            return Ok(());
        }

        order.mov(from, to).map_err(doc_err)?;
        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn apply_delete(&mut self, id: &str) -> Result<(), CoreError> {
        let order = self.doc.get_movable_list("order");
        let ids = order_ids(&order);
        let idx = ids
            .iter()
            .position(|x| x == id)
            .ok_or_else(|| CoreError::NotFound(id.to_string()))?;

        order.delete(idx, 1).map_err(doc_err)?;
        self.doc.get_map("tasks").delete(id).map_err(doc_err)?;

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::clock::FixedClock;

    #[test]
    fn load_rejects_garbage_bytes() {
        let err = Doc::load(1, b"not a valid loro snapshot").err().unwrap();
        assert!(matches!(err, CoreError::Document(_)));
    }

    // The remaining tests bypass `apply` to corrupt the tasks/order
    // invariant directly at the Loro layer — something no sequence of
    // `Command`s can do, but a future CRDT merge can (see the plan's own
    // note on `read()`'s defensive skips). Only this module may do that.

    #[test]
    fn read_skips_order_entry_with_no_matching_task() {
        let doc = Doc::new(1).unwrap();
        doc.doc
            .get_movable_list("order")
            .insert(0, "ghost")
            .unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, &FixedClock(0));
        assert!(snap.rows.is_empty());
    }

    #[test]
    fn read_skips_task_entry_that_is_not_a_map() {
        let doc = Doc::new(1).unwrap();
        // A plain value, not a container created via `ensure_mergeable_map`.
        doc.doc.get_map("tasks").insert("x", 123).unwrap();
        doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, &FixedClock(0));
        assert!(snap.rows.is_empty());
    }

    #[test]
    fn read_skips_task_with_missing_or_wrong_typed_title() {
        let doc = Doc::new(1).unwrap();
        let task = doc.doc.get_map("tasks").ensure_mergeable_map("x").unwrap();
        task.insert("title", 42).unwrap(); // wrong type: I64, not String
        doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, &FixedClock(0));
        assert!(snap.rows.is_empty());
    }

    #[test]
    fn read_defaults_done_to_false_when_wrong_typed() {
        let doc = Doc::new(1).unwrap();
        let task = doc.doc.get_map("tasks").ensure_mergeable_map("x").unwrap();
        task.insert("title", "a task").unwrap();
        task.insert("done", "not a bool").unwrap(); // wrong type: String, not Bool
        doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, &FixedClock(0));
        assert_eq!(snap.rows.len(), 1);
        assert!(!snap.rows[0].done);
    }
}
