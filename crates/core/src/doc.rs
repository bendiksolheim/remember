//! The only module that may mention Loro. Document layout:
//!
//! - `tasks` — a `LoroMap` keyed by task UUID, each value a nested `LoroMap`
//!   with `title`, `notes`, `done`, `due`, `created_at`, `list_id`, and
//!   optionally `parent_id`, the task this one is a step of (see [`Tree`]).
//! - `order` — a `LoroMovableList` of task UUID strings, shared across every
//!   list. Filtering by list just skips non-matching ids in `read()` — the
//!   same trick already used for the Active/Completed filter — so moving a
//!   task within one list reuses `Move` unchanged even though other lists'
//!   tasks are interleaved in the same underlying sequence. A step's
//!   position only matters among its siblings: `read()` always shows steps
//!   right after their task, so they follow it without being moved.
//! - `lists` — a flat `LoroMap` keyed by list UUID, each value its plain
//!   name string (no nested map needed for a single field).
//! - `list_order` — a `LoroMovableList` of list UUID strings, for sidebar
//!   display order.
//! - `list_colors` — a flat `LoroMap` keyed by list UUID, each value the
//!   `u8` index of a [`crate::snapshot::ListColor`] slot. A second flat map
//!   rather than a new field on `lists`' existing string values, so it
//!   layers on without changing what's already there — see
//!   [`bootstrap_list_colors`] for how a list from before this field existed
//!   gets one.
//!
//! No derived state (sort index, cached `due_state`, formatted date) is ever
//! stored in the document — all of that is computed in `read()`. A list's
//! color is the one exception that looks like it could be: it's assigned
//! once and persisted like `name`, not recomputed from scratch on every
//! read — see [`ListColor`]'s own doc comment for why.
//!
//! Every task belongs to exactly one list — `list_id` is never optional at
//! the application level. A task written before lists existed has no
//! `list_id` field at all; `read()` and every list-aware lookup treat a
//! missing field as [`DEFAULT_LIST_ID`], so old data silently lands in the
//! one list [`bootstrap_default_list`] guarantees exists, with no explicit
//! migration pass required.

use std::cmp::Ordering;
use std::collections::HashMap;

use loro::{ExportMode, LoroDoc, LoroMap, LoroMovableList, LoroValue, UndoManager, VersionVector};

use crate::clock::{Clock, IdSource};
use crate::command::{Command, ViewFilter};
use crate::snapshot::{
    due_label, due_state, matches_filter, DueState, ListColor, ListRow, Snapshot, TaskRow,
};
use crate::CoreError;

/// Fixed (not random) so that two devices which have never synced yet and
/// each independently bootstrap a fresh doc converge on the *same* list
/// instead of ending up with two differently-id'd "Tasks" lists once they
/// finally sync. Loro resolves two concurrent writes of the same value to
/// the same map key for free; a random id per device would not have that
/// property.
pub(crate) const DEFAULT_LIST_ID: &str = "default";
const DEFAULT_LIST_NAME: &str = "Tasks";

pub struct Doc {
    doc: LoroDoc,
    undo: UndoManager,
    revision: u64,
    /// The caller's local UTC offset, in seconds — used only to compute
    /// "today" for `read()`'s `due_label`/`due_state` fields. Local display
    /// config, not CRDT content: never committed, never touches `revision`,
    /// never synced. Defaults to 0 (UTC) until the caller sets it — see
    /// `Doc::set_local_offset_seconds`.
    local_offset_seconds: i32,
    /// Imported changes Loro is holding back because they build on changes
    /// this doc has never seen: per peer, the counter they run up to
    /// (exclusive). Loro keeps such changes in memory only, so a reload
    /// starts this empty again — see [`Doc::has_missing_changes`].
    waiting: HashMap<u64, i32>,
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

fn due_to_loro_value(due: Option<i64>) -> LoroValue {
    match due {
        Some(d) => d.into(),
        None => LoroValue::Null,
    }
}

fn decode_vv(bytes: Option<&[u8]>) -> Result<VersionVector, CoreError> {
    match bytes {
        Some(b) => VersionVector::decode(b).map_err(doc_err),
        None => Ok(VersionVector::new()),
    }
}

fn order_ids(order: &LoroMovableList) -> Vec<String> {
    order.to_vec().iter().filter_map(as_string).collect()
}

/// Ids in `list_order`, deduplicated. Two devices that have never synced
/// can each independently bootstrap the default list (see
/// [`DEFAULT_LIST_ID]'s doc comment) and both insert it into their own
/// `list_order` before ever meeting each other — the `lists` map itself
/// converges for free (same key, same value), but `list_order` is a
/// sequence, not a set, so a merge can leave the same id listed twice. This
/// is the same defensive-read posture `read()` already takes toward
/// structural CRDT edge cases, just applied to list order instead of task
/// order.
fn dedup_list_order_ids(list_order: &LoroMovableList) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    order_ids(list_order)
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// Ensures the `lists`/`list_order` containers are non-empty, inserting
/// [`DEFAULT_LIST_ID`] (named [`DEFAULT_LIST_NAME`]) if nothing is there
/// yet. Idempotent and safe to call on every open — see the module doc
/// comment and this function's callers for why it must run before
/// `UndoManager` attaches.
///
/// Checked by emptiness, not by whether `DEFAULT_LIST_ID` specifically is
/// present: a user can delete the bootstrap-created list like any other
/// (see `apply_delete_list`) once a replacement exists, and that deletion
/// must stick across restarts rather than being silently undone here. The
/// last-remaining-list guard in `apply_delete_list` means `lists` can only
/// ever be empty before the very first list — bootstrapped or
/// pre-lists-migrated — has been created, so this is equivalent to "has
/// bootstrap ever run" without needing a separate persisted flag for it.
fn bootstrap_default_list(doc: &LoroDoc) -> Result<(), CoreError> {
    let lists = doc.get_map("lists");
    if !lists.is_empty() {
        return Ok(());
    }
    lists
        .insert(DEFAULT_LIST_ID, DEFAULT_LIST_NAME)
        .map_err(doc_err)?;
    doc.get_movable_list("list_order")
        .insert(0, DEFAULT_LIST_ID)
        .map_err(doc_err)?;
    doc.commit();
    Ok(())
}

/// The lowest [`ListColor`] index not already in `used`. Once every index is
/// taken, reuse is unavoidable — a finite palette can't give more lists than
/// it has slots a color of their own — so this falls back to wrapping
/// around via [`ListColor::from_index`] rather than panicking.
fn next_free_list_color(used: &std::collections::HashSet<u8>) -> ListColor {
    (0..ListColor::ALL.len() as u8)
        .find(|i| !used.contains(i))
        .map_or(
            ListColor::from_index(used.len() as u8),
            ListColor::from_index,
        )
}

/// Every color index already claimed by some other list in `list_colors`,
/// skipping `skip_id` (the list a fresh assignment is being computed for —
/// irrelevant for `apply_add_list`, which hasn't inserted anything yet, but
/// `bootstrap_list_colors` shares this helper while looping over several
/// lists at once and must not let an already-visited list's slot collide
/// with itself).
fn used_list_colors(
    list_colors: &LoroMap,
    ids: &[String],
    skip_id: &str,
) -> std::collections::HashSet<u8> {
    ids.iter()
        .filter(|id| id.as_str() != skip_id)
        .filter_map(|id| {
            list_colors
                .get(id)
                .and_then(|v| as_i64(&v.get_deep_value()))
        })
        .map(|n| n as u8)
        .collect()
}

/// Assigns a persisted color to any list that doesn't have one yet — lists
/// created before this field existed, or the fixed-id default list (created
/// by [`bootstrap_default_list`] directly, not through `apply_add_list`,
/// which is the only other place a color gets assigned). Idempotent and
/// safe to call on every open, same as `bootstrap_default_list`; must run
/// after it for the same before-`UndoManager`-attaches reason. Walks
/// `list_order` so iteration order — and therefore which list gets which
/// leftover slot when more than one is missing — is deterministic.
fn bootstrap_list_colors(doc: &LoroDoc) -> Result<(), CoreError> {
    let list_order = doc.get_movable_list("list_order");
    let list_colors = doc.get_map("list_colors");
    let ids = dedup_list_order_ids(&list_order);

    let missing: Vec<&String> = ids
        .iter()
        .filter(|id| list_colors.get(id.as_str()).is_none())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }

    for id in missing {
        let used = used_list_colors(&list_colors, &ids, id);
        let color = next_free_list_color(&used);
        list_colors
            .insert(id.as_str(), i64::from(color.to_index()))
            .map_err(doc_err)?;
    }
    doc.commit();
    Ok(())
}

/// A task's `list_id`, defaulting to [`DEFAULT_LIST_ID`] when the field is
/// missing (pre-lists data) or wrong-typed — the same defensive-default
/// posture `read()` already takes for `done`.
fn task_list_id(tasks: &LoroMap, id: &str) -> String {
    tasks
        .get(id)
        .and_then(|entry| match entry.get_deep_value() {
            LoroValue::Map(fields) => fields.get("list_id").and_then(as_string),
            _ => None,
        })
        .unwrap_or_else(|| DEFAULT_LIST_ID.to_string())
}

/// One well-formed task as stored.
struct Entry {
    id: String,
    /// Index in `order_ids(order)`.
    pos: usize,
    title: String,
    notes: String,
    done: bool,
    due: Option<i64>,
    list_id: String,
    /// As stored; see [`Tree::new`] for when it counts.
    parent_id: Option<String>,
}

/// Every well-formed task with its steps placed. `read()` and every
/// step-aware command work from this, so they agree on what is a step of
/// what. Indices are into `entries`.
struct Tree {
    /// In `order`.
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    top_level: Vec<usize>,
    /// Per entry, its steps in `order` (always empty for a step).
    steps: Vec<Vec<usize>>,
    /// Per entry, the task it is a step of.
    parent: Vec<Option<usize>>,
}

impl Tree {
    /// A stored `parent_id` only counts if it names another task in the
    /// same list with no stored `parent_id` of its own. Checking the stored
    /// field keeps steps one level deep without cycle detection: a merged
    /// "A under B" and "B under A" both read as top-level.
    fn new(tasks: &LoroMap, order: &LoroMovableList) -> Self {
        let mut entries = Vec::new();
        for (pos, id) in order_ids(order).into_iter().enumerate() {
            let Some(entry) = tasks.get(&id) else {
                continue; // defensive: order references a task `tasks` doesn't have
            };
            let LoroValue::Map(fields) = entry.get_deep_value() else {
                continue; // defensive: entry isn't the expected nested map
            };
            let Some(title) = fields.get("title").and_then(as_string) else {
                continue;
            };
            entries.push(Entry {
                id,
                pos,
                title,
                notes: fields.get("notes").and_then(as_string).unwrap_or_default(),
                done: fields.get("done").and_then(as_bool).unwrap_or(false),
                due: fields.get("due").and_then(as_i64),
                list_id: fields
                    .get("list_id")
                    .and_then(as_string)
                    .unwrap_or_else(|| DEFAULT_LIST_ID.to_string()),
                parent_id: fields.get("parent_id").and_then(as_string),
            });
        }

        let mut index = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            index.entry(entry.id.clone()).or_insert(i);
        }
        let parent: Vec<Option<usize>> = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let p = *index.get(entry.parent_id.as_deref()?)?;
                let candidate = &entries[p];
                (p != i && candidate.parent_id.is_none() && candidate.list_id == entry.list_id)
                    .then_some(p)
            })
            .collect();
        let mut top_level = Vec::new();
        let mut steps = vec![Vec::new(); entries.len()];
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => steps[*p].push(i),
                None => top_level.push(i),
            }
        }
        Tree {
            entries,
            index,
            top_level,
            steps,
            parent,
        }
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    fn id(&self, i: usize) -> &str {
        &self.entries[i].id
    }

    /// The entry a new last step of `task` goes after.
    fn last_in_block(&self, task: usize) -> usize {
        self.steps[task].last().copied().unwrap_or(task)
    }
}

/// Moves a step to just before `first`, its first sibling. `None` when it
/// is already first.
fn resolve_move_first(ids: &[String], id: &str, first: Option<&str>) -> Option<(usize, usize)> {
    let first = first?;
    let from = ids.iter().position(|x| x == id)?;
    let to = ids.iter().position(|x| x == first)?;
    (to < from).then_some((from, to))
}

/// Resolves where `id` should land relative to `after` within `ids` (both
/// drawn from the same movable list), shared by every "reorder relative to
/// a sibling" command (`Move`, `MoveList`). Returns `None` when the
/// resolved position is a no-op (e.g. already first, or already right after
/// `after`) — callers must not commit or bump `revision` in that case, or
/// `Undo` afterward would have nothing of its own to revert.
fn resolve_move(
    ids: &[String],
    id: &str,
    after: Option<&str>,
) -> Result<Option<(usize, usize)>, CoreError> {
    let from = ids
        .iter()
        .position(|x| x == id)
        .ok_or_else(|| CoreError::NotFound(id.to_string()))?;
    let to = match after {
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
    Ok((to != from).then_some((from, to)))
}

impl Doc {
    pub fn new(peer_id: u64) -> Result<Self, CoreError> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(doc_err)?;
        // Default is 1000s: consecutive commits within the window merge into
        // one underlying Change, which breaks "one commit() per user action"
        // — undo would then revert several commands at once.
        doc.set_change_merge_interval(0);
        // Before `UndoManager` attaches: the bootstrap commit must not be
        // undoable, or a user's very first `Undo` on a brand new install
        // would delete the only list that exists.
        bootstrap_default_list(&doc)?;
        bootstrap_list_colors(&doc)?;
        let undo = UndoManager::new(&doc);
        Ok(Self {
            doc,
            undo,
            revision: 0,
            local_offset_seconds: 0,
            waiting: HashMap::new(),
        })
    }

    pub fn load(peer_id: u64, snapshot: &[u8]) -> Result<Self, CoreError> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).map_err(doc_err)?;
        doc.set_change_merge_interval(0);
        doc.import(snapshot).map_err(doc_err)?;
        // Same reasoning as `new`: run before `UndoManager` attaches. Also
        // doubles as the migration path for docs written before lists (or
        // list colors) existed — see the module doc comment.
        bootstrap_default_list(&doc)?;
        bootstrap_list_colors(&doc)?;
        let undo = UndoManager::new(&doc);
        Ok(Self {
            doc,
            undo,
            revision: 0,
            local_offset_seconds: 0,
            waiting: HashMap::new(),
        })
    }

    /// Sets the local UTC offset (seconds) used by `read()` to compute
    /// "today" for `due_label`/`due_state`. Not a CRDT mutation — no commit,
    /// no `revision` bump; the caller (`App`) is responsible for notifying
    /// subscribers itself, since a fresh snapshot with corrected labels is
    /// worth pushing even though the document's content didn't change.
    pub fn set_local_offset_seconds(&mut self, offset_seconds: i32) {
        self.local_offset_seconds = offset_seconds;
    }

    /// The offset most recently set via `set_local_offset_seconds` (0/UTC
    /// until then). Exposed so `App::detect_due` can resolve "today" the
    /// same way `read()` does.
    pub fn local_offset_seconds(&self) -> i32 {
        self.local_offset_seconds
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>, CoreError> {
        self.doc.export(ExportMode::snapshot()).map_err(doc_err)
    }

    /// Full-history update export. Used by the property tests, which want
    /// unconditional convergence regardless of what either side has already
    /// seen. Sync uses [`Doc::export_since`] instead, which is scoped to
    /// what actually changed.
    pub fn export_updates(&self) -> Result<Vec<u8>, CoreError> {
        self.doc.export(ExportMode::all_updates()).map_err(doc_err)
    }

    /// Merges updates from another peer into this doc — the same `import`
    /// primitive `load` uses, just against an already-populated document.
    /// Changes that build on ones this doc doesn't have yet don't fail the
    /// import: Loro holds them back until the missing ones arrive, and
    /// [`Doc::has_missing_changes`] reports it.
    pub fn import_updates(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        let status = self.doc.import(bytes).map_err(doc_err)?;
        for (peer, &(_, end)) in status.pending.iter().flat_map(|range| range.iter()) {
            let waiting_end = self.waiting.entry(*peer).or_insert(end);
            *waiting_end = (*waiting_end).max(end);
        }
        let vv = self.doc.oplog_vv();
        self.waiting
            .retain(|peer, end| vv.get(peer).copied().unwrap_or(0) < *end);
        Ok(())
    }

    /// Whether imported changes are being held back because they build on
    /// changes this doc has never received — another device's history has a
    /// gap here, and nothing it has written since can show until the gap is
    /// filled. Cleared once an import fills it.
    pub fn has_missing_changes(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// This doc's current version vector, encoded. Opaque outside this
    /// module — callers persist it and hand it back to [`Doc::export_since`]
    /// later; nothing outside `doc.rs` interprets its contents.
    pub fn version_vector_bytes(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    /// Like [`Doc::version_vector_bytes`], but as if this doc's own peer had
    /// never made a change: [`Doc::export_since`] it, and the export is this
    /// peer's whole history and nothing else.
    pub fn version_vector_without_own_bytes(&self) -> Vec<u8> {
        let mut vv = self.doc.oplog_vv();
        vv.remove(&self.doc.peer_id());
        vv.encode()
    }

    /// Incremental update export: everything since `since`, not the full
    /// history. `since` is a previously-returned [`Doc::version_vector_bytes`]
    /// result, or `None` to mean "this peer has never exported before" (i.e.
    /// export everything). This is the sync seam `export_updates`'s doc
    /// comment used to call out as deferred — pushes now cost proportional
    /// to what changed, not to total history.
    pub fn export_since(&self, since: Option<&[u8]>) -> Result<Vec<u8>, CoreError> {
        let vv = decode_vv(since)?;
        self.doc.export(ExportMode::updates(&vv)).map_err(doc_err)
    }

    /// Whether anything has happened since `since` (a previous
    /// [`Doc::version_vector_bytes`] result, or `None` for genesis). An
    /// export's *byte length* can't answer this — Loro's update encoding
    /// has a small fixed overhead even when there are zero new ops — so the
    /// sync layer uses this instead to decide whether a push is worth
    /// making at all.
    pub fn has_changes_since(&self, since: Option<&[u8]>) -> Result<bool, CoreError> {
        let baseline = decode_vv(since)?;
        Ok(!baseline.includes_vv(&self.doc.oplog_vv()))
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
            Command::Add {
                title,
                after,
                due,
                list_id,
                parent_id,
            } => match parent_id {
                Some(parent_id) => self.apply_add_step(title, due, &parent_id, clock, ids),
                None => self.apply_add(title, after, due, list_id, clock, ids),
            },
            Command::SetTitle { id, title } => self.apply_set_title(&id, title),
            Command::SetNotes { id, notes } => self.set_field(&id, "notes", notes.into()),
            Command::SetDone { id, done } => self.apply_set_done(&id, done),
            Command::SetParent { id, parent_id } => self.apply_set_parent(&id, parent_id),
            Command::Indent { .. } | Command::Outdent { .. } | Command::Shift { .. } => {
                Err(CoreError::Document(
                    "Indent, Outdent and Shift are resolved by Session, not Doc".to_string(),
                ))
            }
            Command::SetDue { id, due } => self.apply_set_due(&id, due),
            Command::Move { id, after } => self.apply_move(&id, after),
            Command::Delete { id } => self.apply_delete(&id),
            Command::SetList { id, list_id } => self.apply_set_list(&id, list_id),
            Command::AddList { name, after } => self.apply_add_list(name, after, ids),
            Command::RenameList { id, name } => self.apply_rename_list(&id, name),
            Command::DeleteList { id } => self.apply_delete_list(&id),
            Command::MoveList { id, after } => self.apply_move_list(&id, after),
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

    pub fn read(&self, view: ViewFilter, list: &str, clock: &dyn Clock) -> Snapshot {
        let now = clock.now();
        let tasks = self.doc.get_map("tasks");
        let order = self.doc.get_movable_list("order");
        let lists_map = self.doc.get_map("lists");
        let list_colors = self.doc.get_map("list_colors");

        let mut list_rows: Vec<ListRow> =
            dedup_list_order_ids(&self.doc.get_movable_list("list_order"))
                .into_iter()
                .map(|id| {
                    let name = lists_map
                        .get(&id)
                        .and_then(|v| as_string(&v.get_deep_value()))
                        .unwrap_or_default();
                    // Always present by the time `read()` can run — see
                    // `bootstrap_list_colors` — but defaults defensively rather
                    // than panicking if a value is ever missing or wrong-typed,
                    // same posture `task_list_id` takes toward `list_id`.
                    let color = list_colors
                        .get(&id)
                        .and_then(|v| as_i64(&v.get_deep_value()))
                        .map(|n| ListColor::from_index(n as u8))
                        .unwrap_or_default();
                    ListRow {
                        id,
                        name,
                        color,
                        task_count: 0,
                    }
                })
                .collect();

        let tree = Tree::new(&tasks, &order);
        let mut active_count = 0u32;
        let mut task_counts: HashMap<String, u32> = HashMap::new();
        let list_name = lists_map
            .get(list)
            .and_then(|v| as_string(&v.get_deep_value()))
            .unwrap_or_default();
        let row = |entry: &Entry, done: bool, display_due: Option<i64>| TaskRow {
            id: entry.id.clone(),
            title: entry.title.clone(),
            notes: entry.notes.clone(),
            done,
            due: entry.due,
            due_label: display_due.map(|d| due_label(d, now, self.local_offset_seconds)),
            due_state: display_due
                .map(|d| due_state(d, now, self.local_offset_seconds))
                .unwrap_or(DueState::None),
            list_id: entry.list_id.clone(),
            list_name: list_name.clone(),
            parent_id: None,
            step_count: 0,
            steps_done: 0,
            context_only: false,
        };

        let mut rows = Vec::new();
        for &t in &tree.top_level {
            let task = &tree.entries[t];
            let its_steps: Vec<&Entry> = tree.steps[t].iter().map(|&s| &tree.entries[s]).collect();
            let open_steps = its_steps.iter().filter(|s| !s.done);
            let steps_done = its_steps.len() - open_steps.clone().count();
            // A merge can put an open step under a done task; it must stay
            // visible, so the task reads as open.
            let done = task.done && steps_done == its_steps.len();

            for entry in std::iter::once(task).chain(its_steps.iter().copied()) {
                *task_counts.entry(entry.list_id.clone()).or_default() += 1;
            }
            active_count += u32::from(!done) + (its_steps.len() - steps_done) as u32;
            if task.list_id != list {
                continue;
            }

            let shown_steps: Vec<&Entry> = its_steps
                .iter()
                .copied()
                .filter(|s| matches_filter(s.done, view))
                .collect();
            let matches = matches_filter(done, view);
            if !matches && shown_steps.is_empty() {
                continue;
            }

            let next_due = task
                .due
                .into_iter()
                .chain(open_steps.filter_map(|s| s.due))
                .min();
            rows.push(TaskRow {
                step_count: its_steps.len() as u32,
                steps_done: steps_done as u32,
                context_only: !matches,
                ..row(task, done, next_due)
            });
            for step in shown_steps {
                rows.push(TaskRow {
                    parent_id: Some(task.id.clone()),
                    ..row(step, step.done, step.due)
                });
            }
        }

        for list_row in &mut list_rows {
            list_row.task_count = task_counts.get(&list_row.id).copied().unwrap_or(0);
        }

        Snapshot {
            rows,
            view,
            current_list: list.to_string(),
            lists: list_rows,
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
        let value = due_to_loro_value(due);
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
        due: Option<i64>,
        list_id: Option<String>,
        clock: &dyn Clock,
        ids: &dyn IdSource,
    ) -> Result<(), CoreError> {
        let order = self.doc.get_movable_list("order");
        let pos = self.insert_position(&order, after.as_deref())?;

        let list_id = list_id.unwrap_or_else(|| DEFAULT_LIST_ID.to_string());
        if self.doc.get_map("lists").get(&list_id).is_none() {
            return Err(CoreError::NotFound(list_id));
        }

        if self.insert_task(&title, due, &list_id, None, pos, clock, ids)? {
            self.doc.commit();
            self.revision += 1;
        }
        Ok(())
    }

    /// `Add` with a `parent_id`. A done task getting an open step reopens,
    /// as in `apply_set_done`.
    fn apply_add_step(
        &mut self,
        title: String,
        due: Option<i64>,
        parent_id: &str,
        clock: &dyn Clock,
        ids: &dyn IdSource,
    ) -> Result<(), CoreError> {
        let tree = self.tree();
        let p = tree
            .find(parent_id)
            .ok_or_else(|| CoreError::NotFound(parent_id.to_string()))?;
        let task = tree.parent[p].unwrap_or(p);
        let pos = tree.entries[tree.last_in_block(task)].pos + 1;
        let (task_id, list_id) = (tree.id(task), &tree.entries[task].list_id);
        let created = self.insert_task(&title, due, list_id, Some(task_id), pos, clock, ids)?;
        if !created {
            return Ok(());
        }
        if tree.entries[task].done {
            self.existing_task_map(task_id)?
                .insert("done", false)
                .map_err(doc_err)?;
        }
        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    /// Creates a task at `pos` without committing, so the caller's other
    /// writes share its undo step. `false`, doing nothing, for an empty
    /// title (as in apply_set_title).
    #[allow(clippy::too_many_arguments)]
    fn insert_task(
        &mut self,
        title: &str,
        due: Option<i64>,
        list_id: &str,
        parent_id: Option<&str>,
        pos: usize,
        clock: &dyn Clock,
        ids: &dyn IdSource,
    ) -> Result<bool, CoreError> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }

        let id = ids.new_id();
        let tasks = self.doc.get_map("tasks");
        let task = tasks.ensure_mergeable_map(&id).map_err(doc_err)?;
        task.insert("title", trimmed).map_err(doc_err)?;
        task.insert("notes", "").map_err(doc_err)?;
        task.insert("done", false).map_err(doc_err)?;
        task.insert("due", due_to_loro_value(due))
            .map_err(doc_err)?;
        task.insert("created_at", clock.now()).map_err(doc_err)?;
        task.insert("list_id", list_id).map_err(doc_err)?;
        if let Some(parent_id) = parent_id {
            task.insert("parent_id", parent_id).map_err(doc_err)?;
        }

        self.doc
            .get_movable_list("order")
            .insert(pos, id)
            .map_err(doc_err)?;
        Ok(true)
    }

    fn tree(&self) -> Tree {
        Tree::new(
            &self.doc.get_map("tasks"),
            &self.doc.get_movable_list("order"),
        )
    }

    /// A step only moves among its siblings: `after` is one of them, or
    /// `None`/its task for "first". For a top-level task, an `after` naming
    /// a step means after that step's task (e.g. an iOS drop mid-block).
    fn apply_move(&mut self, id: &str, after: Option<String>) -> Result<(), CoreError> {
        let order = self.doc.get_movable_list("order");
        let ids = order_ids(&order);
        let tree = self.tree();
        let step_of = tree.find(id).and_then(|i| tree.parent[i]);
        let resolved = match step_of {
            Some(task) => {
                let siblings: Vec<&str> = tree.steps[task]
                    .iter()
                    .map(|&s| tree.id(s))
                    .filter(|&s| s != id)
                    .collect();
                match after.as_deref() {
                    Some(a) if siblings.contains(&a) => resolve_move(&ids, id, Some(a))?,
                    None => resolve_move_first(&ids, id, siblings.first().copied()),
                    Some(a) if a == tree.id(task) => {
                        resolve_move_first(&ids, id, siblings.first().copied())
                    }
                    Some(_) => {
                        return Err(CoreError::Document(
                            "a step can only move among its task's steps".to_string(),
                        ))
                    }
                }
            }
            None => {
                let after = after.map(|a| match tree.find(&a).and_then(|j| tree.parent[j]) {
                    Some(task) => tree.id(task).to_string(),
                    None => a,
                });
                resolve_move(&ids, id, after.as_deref())?
            }
        };
        let Some((from, to)) = resolved else {
            // Genuinely a no-op (e.g. already first, or already right after
            // the given id) — see the comment in `set_field` for why this
            // must not commit or bump `revision`.
            return Ok(());
        };
        order.mov(from, to).map_err(doc_err)?;
        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    /// Deleting a task deletes its steps with it.
    fn apply_delete(&mut self, id: &str) -> Result<(), CoreError> {
        let order = self.doc.get_movable_list("order");
        let ids = order_ids(&order);
        if !ids.iter().any(|x| x == id) {
            return Err(CoreError::NotFound(id.to_string()));
        }
        let tree = self.tree();
        let mut doomed = vec![id];
        if let Some(i) = tree.find(id) {
            doomed.extend(tree.steps[i].iter().map(|&s| tree.id(s)));
        }

        // Descending indices so deleting one doesn't shift the rest still
        // to be processed.
        let mut indices: Vec<usize> = doomed
            .iter()
            .filter_map(|d| ids.iter().position(|x| x == d))
            .collect();
        indices.sort_unstable();
        for idx in indices.into_iter().rev() {
            order.delete(idx, 1).map_err(doc_err)?;
        }
        let tasks = self.doc.get_map("tasks");
        for task_id in doomed {
            tasks.delete(task_id).map_err(doc_err)?;
        }

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    /// Completing a task completes its steps; reopening a step reopens its
    /// task. Unchanged fields aren't written, so a no-op commits nothing.
    fn apply_set_done(&mut self, id: &str, done: bool) -> Result<(), CoreError> {
        let tree = self.tree();
        let mut targets = vec![id];
        if let Some(i) = tree.find(id) {
            match (done, tree.parent[i]) {
                (true, _) => targets.extend(tree.steps[i].iter().map(|&s| tree.id(s))),
                (false, Some(task)) => targets.push(tree.id(task)),
                (false, None) => {}
            }
        }
        let value = LoroValue::from(done);
        let mut changed = false;
        for target in targets {
            let task = self.existing_task_map(target)?;
            if task.get("done").map(|v| v.get_deep_value()).as_ref() != Some(&value) {
                task.insert("done", value.clone()).map_err(doc_err)?;
                changed = true;
            }
        }
        if changed {
            self.doc.commit();
            self.revision += 1;
        }
        Ok(())
    }

    /// See `Command::SetParent`. Outdenting places the task right after
    /// its former task's own entry in `order`, i.e. below that whole block.
    fn apply_set_parent(&mut self, id: &str, parent_id: Option<String>) -> Result<(), CoreError> {
        let tree = self.tree();
        let i = tree
            .find(id)
            .ok_or_else(|| CoreError::NotFound(id.to_string()))?;
        let order = self.doc.get_movable_list("order");
        let ids = order_ids(&order);
        let task = self.existing_task_map(id)?;

        let after = match parent_id {
            Some(parent_id) => {
                let p = tree
                    .find(&parent_id)
                    .ok_or_else(|| CoreError::NotFound(parent_id.clone()))?;
                if tree.parent[i] == Some(p) {
                    return Ok(());
                }
                if p == i
                    || tree.parent[p].is_some()
                    || tree.entries[p].list_id != tree.entries[i].list_id
                    || !tree.steps[i].is_empty()
                {
                    return Err(CoreError::Document(format!(
                        "{id} can't become a step of {parent_id}"
                    )));
                }
                task.insert("parent_id", parent_id.as_str())
                    .map_err(doc_err)?;
                if !tree.entries[i].done && tree.entries[p].done {
                    self.existing_task_map(&parent_id)?
                        .insert("done", false)
                        .map_err(doc_err)?;
                }
                let last = tree.steps[p]
                    .iter()
                    .copied()
                    .rfind(|&s| s != i)
                    .unwrap_or(p);
                Some(tree.id(last))
            }
            None => {
                if tree.entries[i].parent_id.is_none() {
                    return Ok(());
                }
                // Also clears an ignored stored value, so it can't start
                // counting later.
                task.insert("parent_id", LoroValue::Null).map_err(doc_err)?;
                tree.parent[i].map(|p| tree.id(p))
            }
        };

        if let Some(after) = after {
            if let Some((from, to)) = resolve_move(&ids, id, Some(after))? {
                order.mov(from, to).map_err(doc_err)?;
            }
        }
        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    /// Moves a task to a different list, landing it at global order position
    /// 0. That trivially also puts it first *within* its new list once
    /// `read()` filters by list — no need to search for "the first task
    /// already in that list" — matching the "lands at the top" UX decision.
    /// A task's steps go with it; a step moved on its own stops being one.
    fn apply_set_list(&mut self, id: &str, list_id: String) -> Result<(), CoreError> {
        let task = self.existing_task_map(id)?;
        if self.doc.get_map("lists").get(&list_id).is_none() {
            return Err(CoreError::NotFound(list_id));
        }

        let value: LoroValue = list_id.as_str().into();
        if task.get("list_id").map(|v| v.get_deep_value()).as_ref() == Some(&value) {
            // Same reasoning as `set_field`'s same-value guard.
            return Ok(());
        }
        // Before writing, while its steps still count as its steps.
        let tree = self.tree();
        task.insert("list_id", list_id.as_str()).map_err(doc_err)?;

        if let Some(i) = tree.find(id) {
            // Even an ignored one: it could count in the new list.
            if tree.entries[i].parent_id.is_some() {
                task.insert("parent_id", LoroValue::Null).map_err(doc_err)?;
            }
            for &s in &tree.steps[i] {
                self.existing_task_map(tree.id(s))?
                    .insert("list_id", list_id.as_str())
                    .map_err(doc_err)?;
            }
        }

        let order = self.doc.get_movable_list("order");
        if let Some(from) = order_ids(&order).iter().position(|x| x == id) {
            if from != 0 {
                order.mov(from, 0).map_err(doc_err)?;
            }
        }

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn apply_add_list(
        &mut self,
        name: String,
        after: Option<String>,
        ids: &dyn IdSource,
    ) -> Result<(), CoreError> {
        let list_order = self.doc.get_movable_list("list_order");
        let pos = self.insert_position(&list_order, after.as_deref())?;

        let trimmed = name.trim();
        if trimmed.is_empty() {
            // Mirrors apply_add's empty-title no-op.
            return Ok(());
        }

        let id = ids.new_id();
        self.doc
            .get_map("lists")
            .insert(&id, trimmed)
            .map_err(doc_err)?;

        let list_colors = self.doc.get_map("list_colors");
        let existing_ids = dedup_list_order_ids(&list_order);
        let used = used_list_colors(&list_colors, &existing_ids, &id);
        let color = next_free_list_color(&used);
        list_colors
            .insert(&id, i64::from(color.to_index()))
            .map_err(doc_err)?;

        list_order.insert(pos, id).map_err(doc_err)?;

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn apply_rename_list(&mut self, id: &str, name: String) -> Result<(), CoreError> {
        let lists = self.doc.get_map("lists");
        if lists.get(id).is_none() {
            return Err(CoreError::NotFound(id.to_string()));
        }

        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        let value: LoroValue = trimmed.into();
        if lists.get(id).map(|v| v.get_deep_value()).as_ref() == Some(&value) {
            return Ok(());
        }
        lists.insert(id, trimmed).map_err(doc_err)?;

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    /// Deletes a list and every task in it, atomically. Blocked when this
    /// is the last remaining list — every task must belong to *some* list,
    /// and there'd be nowhere left for new captures to land.
    fn apply_delete_list(&mut self, id: &str) -> Result<(), CoreError> {
        let lists = self.doc.get_map("lists");
        if lists.get(id).is_none() {
            return Err(CoreError::NotFound(id.to_string()));
        }

        let list_order = self.doc.get_movable_list("list_order");
        let distinct_list_count = dedup_list_order_ids(&list_order).len();
        if distinct_list_count <= 1 {
            return Err(CoreError::Document(
                "cannot delete the last remaining list".to_string(),
            ));
        }

        let tasks = self.doc.get_map("tasks");
        let order = self.doc.get_movable_list("order");
        let task_ids = order_ids(&order);

        // Descending indices so deleting one doesn't shift the rest still
        // to be processed.
        let mut doomed_indices = Vec::new();
        let mut doomed_task_ids = Vec::new();
        for (idx, task_id) in task_ids.iter().enumerate() {
            if task_list_id(&tasks, task_id) == id {
                doomed_indices.push(idx);
                doomed_task_ids.push(task_id.clone());
            }
        }
        for idx in doomed_indices.into_iter().rev() {
            order.delete(idx, 1).map_err(doc_err)?;
        }
        for task_id in &doomed_task_ids {
            tasks.delete(task_id).map_err(doc_err)?;
        }

        // Remove every raw occurrence of `id`, not just the first — see
        // `dedup_list_order_ids` for why more than one can exist — in
        // descending index order so earlier indices stay valid as later
        // ones are removed.
        let raw_indices: Vec<usize> = order_ids(&list_order)
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == id)
            .map(|(idx, _)| idx)
            .collect();
        for idx in raw_indices.into_iter().rev() {
            list_order.delete(idx, 1).map_err(doc_err)?;
        }
        lists.delete(id).map_err(doc_err)?;
        // Frees the slot for reuse by a future list rather than leaving it
        // permanently claimed by a list that no longer exists.
        self.doc
            .get_map("list_colors")
            .delete(id)
            .map_err(doc_err)?;

        self.doc.commit();
        self.revision += 1;
        Ok(())
    }

    fn apply_move_list(&mut self, id: &str, after: Option<String>) -> Result<(), CoreError> {
        let list_order = self.doc.get_movable_list("list_order");
        // Deliberately the raw (non-deduplicated) ids: `resolve_move`'s
        // returned indices feed straight into `.mov()` against this same
        // movable list, so they must describe the list exactly as it
        // physically is, not a logically-deduplicated view of it.
        let ids = order_ids(&list_order);
        let Some((from, to)) = resolve_move(&ids, id, after.as_deref())? else {
            return Ok(());
        };
        list_order.mov(from, to).map_err(doc_err)?;
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
    fn bootstrap_backfills_pre_lists_tasks_into_the_default_list() {
        // Simulate data written before lists existed: a task with no
        // `list_id` field at all, built below the `apply`/`Command` layer
        // the same way the other defensive tests in this module do.
        let pre_lists_doc = LoroDoc::new();
        pre_lists_doc.set_peer_id(1).unwrap();
        let task = pre_lists_doc
            .get_map("tasks")
            .ensure_mergeable_map("x")
            .unwrap();
        task.insert("title", "old task").unwrap();
        task.insert("notes", "").unwrap();
        task.insert("done", false).unwrap();
        task.insert("due", LoroValue::Null).unwrap();
        pre_lists_doc
            .get_movable_list("order")
            .insert(0, "x")
            .unwrap();
        pre_lists_doc.commit();
        let snapshot = pre_lists_doc.export(ExportMode::snapshot()).unwrap();

        let doc = Doc::load(2, &snapshot).unwrap();
        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists.len(), 1);
        assert_eq!(snap.lists[0].id, DEFAULT_LIST_ID);
        assert_eq!(snap.rows[0].list_id, DEFAULT_LIST_ID);
        assert_eq!(snap.rows[0].list_name, DEFAULT_LIST_NAME);
    }

    #[test]
    fn dedup_list_order_ids_collapses_duplicate_entries() {
        // Simulates two never-synced devices each independently
        // bootstrapping the default list, then syncing — see
        // `DEFAULT_LIST_ID`'s own doc comment for why this is possible and
        // `dedup_list_order_ids`'s for why `read()` must tolerate it.
        let doc = Doc::new(1).unwrap();
        doc.doc
            .get_movable_list("list_order")
            .insert(0, DEFAULT_LIST_ID)
            .unwrap();
        doc.doc.commit();

        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists.len(), 1);
    }

    #[test]
    fn delete_list_sweeps_up_a_task_whose_entry_is_not_a_map() {
        // Mirrors `read_skips_task_entry_that_is_not_a_map`: a plain value,
        // not a container, so `task_list_id` falls back to the default list
        // — and a `DeleteList("default")` must still account for it.
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        doc.doc.get_map("tasks").insert("x", 123).unwrap();
        doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        doc.doc.commit();

        doc.apply(
            Command::DeleteList {
                id: DEFAULT_LIST_ID.to_string(),
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        assert!(doc.doc.get_map("tasks").get("x").is_none());
        assert!(order_ids(&doc.doc.get_movable_list("order")).is_empty());
    }

    #[test]
    fn set_list_on_a_task_already_first_skips_the_reposition() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        let work = doc
            .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
            .lists[0]
            .id
            .clone();
        let a = doc
            .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
            .rows[0]
            .id
            .clone();

        // `a` is already at order position 0 — changing its list must still
        // succeed without needing to move it anywhere.
        doc.apply(
            Command::SetList {
                id: a.clone(),
                list_id: work.clone(),
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        let snap = doc.read(ViewFilter::All, &work, &FixedClock(0));
        assert_eq!(snap.rows[0].id, a);
        assert_eq!(snap.rows[0].list_id, work);
    }

    /// Adds "Work" and returns its id.
    fn add_work_list(doc: &mut Doc, ids: &crate::clock::SeqIdSource) -> String {
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            ids,
        )
        .unwrap();
        // `after: None` inserts at the top, so "Work" is lists[0].
        doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
            .lists[0]
            .id
            .clone()
    }

    /// Adds a task at the top of `list_id` and returns its id.
    fn add_task(
        doc: &mut Doc,
        ids: &crate::clock::SeqIdSource,
        title: &str,
        list_id: &str,
    ) -> String {
        doc.apply(
            Command::Add {
                title: title.to_string(),
                after: None,
                due: None,
                list_id: Some(list_id.to_string()),
                parent_id: None,
            },
            &FixedClock(0),
            ids,
        )
        .unwrap();
        doc.read(ViewFilter::All, list_id, &FixedClock(0)).rows[0]
            .id
            .clone()
    }

    #[test]
    fn delete_list_takes_its_tasks_with_it_and_leaves_the_rest() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        let work = add_work_list(&mut doc, &ids);
        add_task(&mut doc, &ids, "work task", &work);
        let kept = add_task(&mut doc, &ids, "default task", DEFAULT_LIST_ID);
        let titles = |doc: &Doc| -> Vec<String> {
            doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
                .rows
                .into_iter()
                .map(|r| r.title)
                .collect()
        };
        // Reading one list leaves out the other's tasks.
        assert_eq!(titles(&doc), vec!["default task"]);

        doc.apply(Command::DeleteList { id: work }, &FixedClock(0), &ids)
            .unwrap();

        assert_eq!(titles(&doc), vec!["default task"]);
        assert_eq!(order_ids(&doc.doc.get_movable_list("order")), vec![kept]);
    }

    #[test]
    fn set_list_to_an_unknown_list_is_not_found() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        let a = add_task(&mut doc, &ids, "a", DEFAULT_LIST_ID);

        let result = doc.apply(
            Command::SetList {
                id: a,
                list_id: "nope".to_string(),
            },
            &FixedClock(0),
            &ids,
        );

        assert!(matches!(result, Err(CoreError::NotFound(id)) if id == "nope"));
    }

    #[test]
    fn set_list_to_the_list_a_task_is_already_in_commits_nothing() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        let work = add_work_list(&mut doc, &ids);
        let a = add_task(&mut doc, &ids, "a", &work);
        let revision = doc.revision;

        doc.apply(
            Command::SetList {
                id: a,
                list_id: work,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        assert_eq!(doc.revision, revision);
    }

    #[test]
    fn set_list_moves_a_task_to_the_top() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        let work = add_work_list(&mut doc, &ids);
        let a = add_task(&mut doc, &ids, "a", DEFAULT_LIST_ID);
        add_task(&mut doc, &ids, "b", DEFAULT_LIST_ID); // now above `a`

        doc.apply(
            Command::SetList {
                id: a.clone(),
                list_id: work,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        assert_eq!(order_ids(&doc.doc.get_movable_list("order"))[0], a);
    }

    #[test]
    fn set_list_on_a_task_missing_from_the_order_still_changes_its_list() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        // A task entry with no order entry, as a merge can leave behind.
        let task = doc.doc.get_map("tasks").ensure_mergeable_map("x").unwrap();
        task.insert("title", "a task").unwrap();
        doc.doc.commit();
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        let work = doc
            .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
            .lists[0]
            .id
            .clone();

        doc.apply(
            Command::SetList {
                id: "x".to_string(),
                list_id: work.clone(),
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        assert_eq!(
            task.get("list_id").unwrap().get_deep_value(),
            work.as_str().into()
        );
        assert!(order_ids(&doc.doc.get_movable_list("order")).is_empty());
    }

    #[test]
    fn add_list_assigns_each_list_a_distinct_color_up_to_the_palette_size() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        // The bootstrap default list ("Tasks") already claimed one slot, so
        // 8 more fills the remaining 8 of 9 without any collision.
        for name in ["Bekk", "DA", "Privat", "A", "B", "C", "D", "E"] {
            doc.apply(
                Command::AddList {
                    name: name.to_string(),
                    after: None,
                },
                &FixedClock(0),
                &ids,
            )
            .unwrap();
        }

        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists.len(), 9);
        let colors: std::collections::HashSet<ListColor> =
            snap.lists.iter().map(|l| l.color).collect();
        assert_eq!(colors.len(), 9, "every list must get a distinct color");
    }

    #[test]
    fn add_list_reuses_a_color_once_the_palette_is_exhausted() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        for name in ["Bekk", "DA", "Privat", "A", "B", "C", "D", "E", "Tenth"] {
            doc.apply(
                Command::AddList {
                    name: name.to_string(),
                    after: None,
                },
                &FixedClock(0),
                &ids,
            )
            .unwrap();
        }

        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists.len(), 10);
        // The 10th list (index 9 in creation order, right after the
        // bootstrap default list) has nowhere new to go — same color as the
        // first, by `next_free_list_color`'s documented wrap-around.
        assert_eq!(snap.lists[0].color, snap.lists[9].color);
    }

    #[test]
    fn delete_list_frees_its_color_for_reuse() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        // `after: None` inserts at the top (see `insert_position`), so
        // "Work" is lists[0] and the bootstrap default list is lists[1].
        let before = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        let work_id = before.lists[0].id.clone();
        let work_color = before.lists[0].color;

        doc.apply(Command::DeleteList { id: work_id }, &FixedClock(0), &ids)
            .unwrap();
        doc.apply(
            Command::AddList {
                name: "Replacement".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists.len(), 2);
        assert_eq!(snap.lists[0].color, work_color);
    }

    #[test]
    fn deleting_the_default_list_does_not_resurrect_it_on_reload() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        doc.apply(
            Command::DeleteList {
                id: DEFAULT_LIST_ID.to_string(),
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        let snapshot = doc.export_snapshot().unwrap();
        let reloaded = Doc::load(2, &snapshot).unwrap();
        let snap = reloaded.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));

        assert_eq!(snap.lists.len(), 1);
        assert_eq!(snap.lists[0].name, "Work");
        assert!(snap.lists.iter().all(|l| l.id != DEFAULT_LIST_ID));
    }

    #[test]
    fn bootstrap_backfills_colors_for_lists_written_before_the_field_existed() {
        // Simulate data written before `list_colors` existed: two lists
        // present in `lists`/`list_order`, built below the `apply`/`Command`
        // layer the same way `bootstrap_backfills_pre_lists_tasks_into_the_default_list`
        // simulates pre-lists data. Uses `DEFAULT_LIST_ID` for one of them so
        // `bootstrap_default_list` is a no-op here too, matching what real
        // pre-`list_colors` history actually looks like (lists already
        // existed; only the color map is new).
        let pre_colors_doc = LoroDoc::new();
        pre_colors_doc.set_peer_id(1).unwrap();
        let lists = pre_colors_doc.get_map("lists");
        lists.insert(DEFAULT_LIST_ID, DEFAULT_LIST_NAME).unwrap();
        lists.insert("b", "Beta").unwrap();
        let list_order = pre_colors_doc.get_movable_list("list_order");
        list_order.insert(0, DEFAULT_LIST_ID).unwrap();
        list_order.insert(1, "b").unwrap();
        pre_colors_doc.commit();
        let snapshot = pre_colors_doc.export(ExportMode::snapshot()).unwrap();

        let doc = Doc::load(2, &snapshot).unwrap();
        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists.len(), 2);
        assert_ne!(snap.lists[0].color, snap.lists[1].color);
    }

    #[test]
    fn delete_list_self_heals_duplicate_list_order_entries() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        doc.apply(
            Command::AddList {
                name: "Work".to_string(),
                after: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        // Corrupt the structure: a second raw "default" entry, as a merge
        // of two independently-bootstrapped devices could produce.
        doc.doc
            .get_movable_list("list_order")
            .insert(0, DEFAULT_LIST_ID)
            .unwrap();
        doc.doc.commit();

        doc.apply(
            Command::DeleteList {
                id: DEFAULT_LIST_ID.to_string(),
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        let raw_ids = order_ids(&doc.doc.get_movable_list("list_order"));
        assert!(!raw_ids.iter().any(|id| id == DEFAULT_LIST_ID));
    }

    #[test]
    fn read_skips_order_entry_with_no_matching_task() {
        let doc = Doc::new(1).unwrap();
        doc.doc
            .get_movable_list("order")
            .insert(0, "ghost")
            .unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert!(snap.rows.is_empty());
    }

    #[test]
    fn read_skips_task_entry_that_is_not_a_map() {
        let doc = Doc::new(1).unwrap();
        // A plain value, not a container created via `ensure_mergeable_map`.
        doc.doc.get_map("tasks").insert("x", 123).unwrap();
        doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert!(snap.rows.is_empty());
    }

    #[test]
    fn read_skips_task_with_missing_or_wrong_typed_title() {
        let doc = Doc::new(1).unwrap();
        let task = doc.doc.get_map("tasks").ensure_mergeable_map("x").unwrap();
        task.insert("title", 42).unwrap(); // wrong type: I64, not String
        doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        doc.doc.commit();
        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
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
        let snap = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.rows.len(), 1);
        assert!(!snap.rows[0].done);
    }

    #[test]
    fn export_since_none_exports_full_history() {
        let mut doc = Doc::new(1).unwrap();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &crate::clock::SeqIdSource::new(),
        )
        .unwrap();

        let bytes = doc.export_since(None).unwrap();

        let mut other = Doc::new(2).unwrap();
        other.import_updates(&bytes).unwrap();
        assert_eq!(other.task_count(), 1);
        assert_eq!(other.order_len(), 1);
    }

    #[test]
    fn export_since_current_vv_is_empty_delta() {
        let mut doc = Doc::new(1).unwrap();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &crate::clock::SeqIdSource::new(),
        )
        .unwrap();

        let vv = doc.version_vector_bytes();
        let bytes = doc.export_since(Some(&vv)).unwrap();

        // Nothing new since `vv` — importing it into a fresh doc adds no
        // tasks, proving the export was scoped rather than falling back to
        // full history.
        let mut other = Doc::new(2).unwrap();
        other.import_updates(&bytes).unwrap();
        assert_eq!(other.task_count(), 0);
    }

    #[test]
    fn export_since_only_includes_changes_after_the_given_vv() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();
        let vv_after_first = doc.version_vector_bytes();
        let base = doc.export_since(None).unwrap(); // full state as of "a" alone

        doc.apply(
            Command::Add {
                title: "b".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &ids,
        )
        .unwrap();

        let delta = doc.export_since(Some(&vv_after_first)).unwrap();
        let full = doc.export_since(None).unwrap();
        // The delta is strictly smaller than a full re-export of the same
        // final state — proof it's scoped to what's new, not a redundant
        // copy of everything.
        assert!(delta.len() < full.len());

        // Applied on top of the state it was scoped against, the delta
        // reconstructs the full document.
        let mut other = Doc::new(2).unwrap();
        other.import_updates(&base).unwrap();
        assert_eq!(other.task_count(), 1);
        other.import_updates(&delta).unwrap();
        assert_eq!(other.task_count(), 2);
    }

    /// Even before any `Command` is ever applied, a fresh doc already has
    /// content — `bootstrap_default_list`'s own commit — so this is `true`
    /// from the very start, not only "once anything happened" via `apply`.
    #[test]
    fn has_changes_since_none_is_true_immediately_after_bootstrap() {
        let doc = Doc::new(1).unwrap();
        assert!(doc.has_changes_since(None).unwrap());
    }

    #[test]
    fn has_changes_since_current_vv_is_false() {
        let mut doc = Doc::new(1).unwrap();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &crate::clock::SeqIdSource::new(),
        )
        .unwrap();

        assert!(doc.has_changes_since(None).unwrap());
        let vv = doc.version_vector_bytes();
        assert!(!doc.has_changes_since(Some(&vv)).unwrap());
    }

    #[test]
    fn has_changes_since_rejects_garbage_vv_bytes() {
        let doc = Doc::new(1).unwrap();
        let err = doc
            .has_changes_since(Some(b"not a version vector"))
            .err()
            .unwrap();
        assert!(matches!(err, CoreError::Document(_)));
    }

    #[test]
    fn export_since_rejects_garbage_vv_bytes() {
        let doc = Doc::new(1).unwrap();
        let err = doc
            .export_since(Some(b"not a version vector"))
            .err()
            .unwrap();
        assert!(matches!(err, CoreError::Document(_)));
    }

    #[test]
    fn add_with_due_stores_it_on_the_new_task() {
        let mut doc = Doc::new(1).unwrap();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: Some(1_000),
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &crate::clock::SeqIdSource::new(),
        )
        .unwrap();
        let row = &doc
            .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
            .rows[0];
        assert_eq!(row.due, Some(1_000));
    }

    #[test]
    fn add_with_no_due_leaves_it_unset() {
        let mut doc = Doc::new(1).unwrap();
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: None,
                parent_id: None,
            },
            &FixedClock(0),
            &crate::clock::SeqIdSource::new(),
        )
        .unwrap();
        let row = &doc
            .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
            .rows[0];
        assert_eq!(row.due, None);
    }

    #[test]
    fn local_offset_seconds_defaults_to_zero_and_is_settable() {
        let mut doc = Doc::new(1).unwrap();
        assert_eq!(doc.local_offset_seconds(), 0);
        doc.set_local_offset_seconds(-28_800);
        assert_eq!(doc.local_offset_seconds(), -28_800);
    }

    #[test]
    fn set_local_offset_seconds_changes_read_without_committing_or_bumping_revision() {
        let mut doc = Doc::new(1).unwrap();
        let ids = crate::clock::SeqIdSource::new();
        // due = 1970-01-01 00:00 UTC; "now" = 1970-01-01 13:00 UTC — same
        // UTC calendar day, so at offset 0 this isn't overdue.
        let clock = FixedClock(46_800);
        doc.apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: Some(0),
                list_id: None,
                parent_id: None,
            },
            &clock,
            &ids,
        )
        .unwrap();
        let before = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &clock);
        assert_eq!(before.rows[0].due_state, DueState::Today);

        // UTC+12: "now" (13:00 UTC) is already 1970-01-02 01:00 local — a
        // full local day past `due`, which is still 1970-01-01 12:00 local
        // at that same offset. Overdue flips without any new command or a
        // `revision` bump, since this is display config, not content.
        let revision_before = before.revision;
        doc.set_local_offset_seconds(12 * 3_600);
        let after = doc.read(ViewFilter::All, DEFAULT_LIST_ID, &clock);
        assert_eq!(after.rows[0].due_state, DueState::Overdue);
        assert_eq!(after.revision, revision_before);
    }

    // --- Steps -----------------------------------------------------------

    /// A doc and its id source, with shorthands for the step tests below.
    struct Fixture {
        doc: Doc,
        ids: crate::clock::SeqIdSource,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                doc: Doc::new(1).unwrap(),
                ids: crate::clock::SeqIdSource::new(),
            }
        }

        fn run(&mut self, cmd: Command) -> Result<(), CoreError> {
            self.doc.apply(cmd, &FixedClock(0), &self.ids)
        }

        fn ok(&mut self, cmd: Command) {
            self.run(cmd).unwrap();
        }

        fn work_list(&mut self) -> String {
            add_work_list(&mut self.doc, &self.ids)
        }

        /// Adds a task at the top of the default list.
        fn task(&mut self, title: &str) -> String {
            add_task(&mut self.doc, &self.ids, title, DEFAULT_LIST_ID)
        }

        fn task_in(&mut self, title: &str, list: &str) -> String {
            add_task(&mut self.doc, &self.ids, title, list)
        }

        fn step(&mut self, title: &str, parent: &str) -> String {
            self.ok(add_step(title, parent));
            let tree = self.doc.tree();
            let newest = tree
                .entries
                .iter()
                .max_by_key(|e| e.id.parse::<u64>().unwrap())
                .unwrap();
            assert_eq!(newest.title, title);
            newest.id.clone()
        }

        fn raw(&self, id: &str, field: &str, value: impl Into<LoroValue>) {
            set_raw_field(&self.doc, id, field, value);
        }

        fn view_in(&self, view: ViewFilter, list: &str) -> Vec<String> {
            outline(&self.doc.read(view, list, &FixedClock(0)))
        }

        fn view(&self, view: ViewFilter) -> Vec<String> {
            self.view_in(view, DEFAULT_LIST_ID)
        }

        fn outline(&self) -> Vec<String> {
            self.view(ViewFilter::All)
        }

        fn rows(&self) -> Vec<TaskRow> {
            self.doc
                .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
                .rows
        }

        fn done(&self) -> Vec<bool> {
            self.rows().iter().map(|r| r.done).collect()
        }

        fn revision(&self) -> u64 {
            self.doc
                .read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0))
                .revision
        }
    }

    /// Writes a task field straight into the document, bypassing commands —
    /// to exercise `read()`'s repair rules and to build merge states.
    fn set_raw_field(doc: &Doc, id: &str, field: &str, value: impl Into<LoroValue>) {
        doc.existing_task_map(id)
            .unwrap()
            .insert(field, value.into())
            .unwrap();
        doc.doc.commit();
    }

    /// Titles in display order: steps indented, context rows in brackets,
    /// and a task's progress after it.
    fn outline(snap: &Snapshot) -> Vec<String> {
        snap.rows
            .iter()
            .map(|r| {
                let title = match (r.context_only, r.step_count) {
                    (true, _) => format!("({})", r.title),
                    (false, 0) => r.title.clone(),
                    (false, n) => format!("{} {}/{n}", r.title, r.steps_done),
                };
                match r.parent_id {
                    Some(_) => format!("  {title}"),
                    None => title,
                }
            })
            .collect()
    }

    fn add_step(title: &str, parent: &str) -> Command {
        Command::Add {
            title: title.to_string(),
            after: None,
            due: None,
            list_id: None,
            parent_id: Some(parent.to_string()),
        }
    }

    fn done(id: &str, done: bool) -> Command {
        let id = id.to_string();
        Command::SetDone { id, done }
    }

    fn parent(id: &str, parent: Option<&str>) -> Command {
        let (id, parent_id) = (id.to_string(), parent.map(str::to_string));
        Command::SetParent { id, parent_id }
    }

    fn mv(id: &str, after: Option<&str>) -> Command {
        let (id, after) = (id.to_string(), after.map(str::to_string));
        Command::Move { id, after }
    }

    fn set_list(id: &str, list: &str) -> Command {
        let (id, list_id) = (id.to_string(), list.to_string());
        Command::SetList { id, list_id }
    }

    fn delete(id: &str) -> Command {
        Command::Delete { id: id.to_string() }
    }

    fn not_found(result: Result<(), CoreError>) -> bool {
        matches!(result, Err(CoreError::NotFound(_)))
    }

    fn rejected(result: Result<(), CoreError>) -> bool {
        matches!(result, Err(CoreError::Document(_)))
    }

    #[test]
    fn read_places_steps_directly_under_their_task() {
        let mut t = Fixture::new();
        let p = t.task("p");
        let s1 = t.task("s1");
        t.task("x");
        let s2 = t.task("s2");
        // `order` is now s2, x, s1, p: both steps above their task, with an
        // unrelated task between them.
        t.raw(&s1, "parent_id", p.as_str());
        t.raw(&s2, "parent_id", p.as_str());
        t.raw(&s2, "done", true);

        assert_eq!(t.outline(), ["x", "p 1/2", "  s2", "  s1"]);
        assert_eq!(t.rows()[2].parent_id.as_deref(), Some(p.as_str()));
        let snap = t.doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.lists[0].task_count, 4);
    }

    #[test]
    fn read_ignores_a_parent_id_that_does_not_hold_up() {
        let mut t = Fixture::new();
        let work = t.work_list();
        let missing = t.task("missing");
        t.raw(&missing, "parent_id", "nope");
        let itself = t.task("itself");
        t.raw(&itself, "parent_id", itself.as_str());
        let elsewhere = t.task_in("elsewhere", &work);
        let other_list = t.task("other list");
        t.raw(&other_list, "parent_id", elsewhere.as_str());
        let wrong_type = t.task("wrong type");
        t.raw(&wrong_type, "parent_id", 5);
        // h under i and i under h, as two devices could each have written.
        let (h, i) = (t.task("h"), t.task("i"));
        t.raw(&h, "parent_id", i.as_str());
        t.raw(&i, "parent_id", h.as_str());
        // g under f under e: only f is a step; a step can't have steps.
        let (e, f, g) = (t.task("e"), t.task("f"), t.task("g"));
        t.raw(&f, "parent_id", e.as_str());
        t.raw(&g, "parent_id", f.as_str());

        assert_eq!(
            t.outline(),
            [
                "g",
                "e 0/1",
                "  f",
                "i",
                "h",
                "wrong type",
                "other list",
                "itself",
                "missing"
            ]
        );
        assert_eq!(t.view_in(ViewFilter::All, &work), ["elsewhere"]);
    }

    #[test]
    fn a_task_stored_done_reads_open_while_a_step_is_open() {
        let mut t = Fixture::new();
        let task = t.task("task");
        t.step("step", &task);
        let mut b = Doc::load(2, &t.doc.export_snapshot().unwrap()).unwrap();

        // Device a completes the task and its step; device b, offline, adds
        // another step to it. Then they sync.
        t.ok(done(&task, true));
        assert_eq!(t.done(), [true, true]);
        let late = add_task(&mut b, &t.ids, "late step", DEFAULT_LIST_ID);
        set_raw_field(&b, &late, "parent_id", task.as_str());
        t.doc.import_updates(&b.export_updates().unwrap()).unwrap();

        assert_eq!(t.outline(), ["task 1/2", "  late step", "  step"]);
        assert_eq!(t.done(), [false, false, true]);
        assert_eq!(t.view(ViewFilter::Active), ["task 1/2", "  late step"]);
        let snap = t.doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.active_count, 2);

        // Once the last step is done too, the stored value counts again.
        t.raw(&late, "done", true);
        assert_eq!(t.done(), [true, true, true]);
        let snap = t.doc.read(ViewFilter::All, DEFAULT_LIST_ID, &FixedClock(0));
        assert_eq!(snap.active_count, 0);
        assert_eq!(t.rows()[0].id, task);
    }

    #[test]
    fn views_show_a_task_for_its_matching_steps() {
        let mut t = Fixture::new();
        // half: one step done, one open. finished: its only step done.
        // closed: done along with its step.
        let half = t.task("half");
        let half_done = t.task("half done");
        let half_open = t.task("half open");
        let finished = t.task("finished");
        let finished_step = t.task("finished step");
        let closed = t.task("closed");
        let closed_step = t.task("closed step");
        for (step, task) in [
            (&half_done, &half),
            (&half_open, &half),
            (&finished_step, &finished),
            (&closed_step, &closed),
        ] {
            t.raw(step, "parent_id", task.as_str());
        }
        for id in [&half_done, &finished_step, &closed, &closed_step] {
            t.raw(id, "done", true);
        }

        assert_eq!(
            t.view(ViewFilter::Active),
            ["finished 1/1", "half 1/2", "  half open"]
        );
        assert_eq!(
            t.view(ViewFilter::Completed),
            [
                "closed 1/1",
                "  closed step",
                "(finished)",
                "  finished step",
                "(half)",
                "  half done"
            ]
        );
        assert_eq!(t.outline().len(), 7);
    }

    #[test]
    fn a_task_with_steps_shows_its_next_due_date() {
        const DAY: i64 = crate::civil::SECONDS_PER_DAY;
        let mut t = Fixture::new();
        let task = t.task("task");
        let soon = t.step("soon", &task);
        let later = t.step("later", &task);
        t.step("undated", &task);
        t.raw(&soon, "due", 0);
        t.raw(&later, "due", DAY);
        let due = |t: &Fixture| -> Vec<(Option<i64>, Option<String>, DueState)> {
            let rows = t.rows();
            rows.into_iter()
                .map(|r| (r.due, r.due_label, r.due_state))
                .collect()
        };
        let today = || Some("Today".to_string());
        let tomorrow = || Some("Tomorrow".to_string());

        // No own date: the soonest open step's. Steps keep their own.
        assert_eq!(
            due(&t),
            [
                (None, today(), DueState::Today),
                (Some(0), today(), DueState::Today),
                (Some(DAY), tomorrow(), DueState::Later),
                (None, None, DueState::None),
            ]
        );
        // A done step's date no longer counts.
        t.raw(&soon, "done", true);
        assert_eq!(due(&t)[0], (None, tomorrow(), DueState::Later));
        // An own date counts when it's sooner; `due` stays the raw one.
        t.raw(&task, "due", -DAY);
        assert_eq!(
            due(&t)[0],
            (Some(-DAY), Some("31 Dec".to_string()), DueState::Overdue)
        );
        t.raw(&task, "due", 10 * DAY);
        assert_eq!(due(&t)[0], (Some(10 * DAY), tomorrow(), DueState::Later));
    }

    #[test]
    fn add_step_lands_last_in_its_task_and_its_list() {
        let mut t = Fixture::new();
        let work = t.work_list();
        let task = t.task_in("task", &work);
        t.task_in("other", &work);
        let first = t.step("first", &task);
        // Naming a step adds to that step's task.
        t.step("second", &first);
        assert_eq!(
            t.view_in(ViewFilter::All, &work),
            ["other", "task 0/2", "  first", "  second"]
        );
        assert!(t.outline().is_empty());

        let revision = t.revision();
        t.ok(add_step("  ", &task));
        assert_eq!(t.revision(), revision);
        assert!(not_found(t.run(add_step("x", "gone"))));
    }

    #[test]
    fn add_step_to_a_done_task_reopens_it_in_one_undo_step() {
        let mut t = Fixture::new();
        let task = t.task("task");
        t.ok(done(&task, true));
        t.step("step", &task);
        assert_eq!(t.done(), [false, false]);
        let stored = t.doc.existing_task_map(&task).unwrap().get("done").unwrap();
        assert_eq!(stored.get_deep_value(), false.into());

        t.ok(Command::Undo);
        assert_eq!(t.outline(), ["task"]);
        assert_eq!(t.done(), [true]);
    }

    #[test]
    fn set_parent_indents_to_last_step_and_outdents_to_just_below() {
        let mut t = Fixture::new();
        let x = t.task("x");
        let task = t.task("task");
        let s1 = t.step("s1", &task);
        t.task("y");
        // Moving the task to the top leaves its step's entry below y in
        // `order`, which must not decide where an outdented step lands.
        t.ok(mv(&task, None));
        assert_eq!(t.outline(), ["task 0/1", "  s1", "y", "x"]);

        t.ok(parent(&x, Some(&task)));
        assert_eq!(t.outline(), ["task 0/2", "  s1", "  x", "y"]);

        let revision = t.revision();
        t.ok(parent(&x, Some(&task)));
        t.ok(parent(&task, None));
        assert_eq!(t.revision(), revision);

        t.ok(parent(&s1, None));
        assert_eq!(t.outline(), ["task 0/1", "  x", "s1", "y"]);
        t.ok(Command::Undo);
        assert_eq!(t.outline(), ["task 0/2", "  s1", "  x", "y"]);
    }

    #[test]
    fn set_parent_rejects_what_would_break_the_tree() {
        let mut t = Fixture::new();
        let work = t.work_list();
        let elsewhere = t.task_in("elsewhere", &work);
        let plain = t.task("plain");
        let task = t.task("task");
        let step = t.step("step", &task);

        assert!(rejected(t.run(parent(&plain, Some(&elsewhere))))); // another list
        assert!(rejected(t.run(parent(&plain, Some(&step))))); // a step can't have steps
        assert!(rejected(t.run(parent(&task, Some(&plain))))); // has steps of its own
        assert!(rejected(t.run(parent(&plain, Some(&plain))))); // itself
        assert!(not_found(t.run(parent("gone", None))));
        assert!(not_found(t.run(parent(&plain, Some("gone")))));
    }

    #[test]
    fn indenting_an_open_task_under_a_done_one_reopens_it() {
        let mut t = Fixture::new();
        let open = t.task("open");
        let closed = t.task("closed");
        let finished = t.task("finished");
        t.ok(done(&closed, true));
        t.ok(done(&finished, true));

        // A done task joining stays done.
        t.ok(parent(&finished, Some(&closed)));
        assert!(t.done()[0]);
        t.ok(parent(&open, Some(&closed)));
        assert!(!t.done()[0]);
    }

    #[test]
    fn outdent_clears_a_stored_parent_read_was_ignoring() {
        let mut t = Fixture::new();
        let work = t.work_list();
        let target = t.task_in("target", &work);
        let stray = t.task("stray");
        t.raw(&stray, "parent_id", target.as_str());

        let revision = t.revision();
        t.ok(parent(&stray, None));
        assert!(t.revision() > revision);
        // Bypasses `SetList` (which clears it too) to show the field is gone.
        t.raw(&target, "list_id", DEFAULT_LIST_ID);
        assert_eq!(t.outline(), ["stray", "target"]);
    }

    #[test]
    fn completing_a_task_completes_its_steps_in_one_undo_step() {
        let mut t = Fixture::new();
        let task = t.task("task");
        let a = t.step("a", &task);
        t.step("b", &task);
        t.ok(done(&a, true));

        t.ok(done(&task, true));
        assert_eq!(t.done(), [true, true, true]);
        let revision = t.revision();
        t.ok(done(&task, true));
        assert_eq!(t.revision(), revision);

        t.ok(Command::Undo);
        assert_eq!(t.done(), [false, true, false]);
    }

    #[test]
    fn steps_and_their_task_complete_and_reopen_by_the_agreed_rules() {
        let mut t = Fixture::new();
        let task = t.task("task");
        let a = t.step("a", &task);

        // The last step done leaves the task open.
        t.ok(done(&a, true));
        assert_eq!(t.done(), [false, true]);
        // Reopening the task leaves its steps alone.
        t.ok(done(&task, true));
        t.ok(done(&task, false));
        assert_eq!(t.done(), [false, true]);
        // Reopening a step reopens its task, as one undo step.
        t.ok(done(&task, true));
        t.ok(done(&a, false));
        assert_eq!(t.done(), [false, false]);
        t.ok(Command::Undo);
        assert_eq!(t.done(), [true, true]);

        assert!(not_found(t.run(done("gone", true))));
    }

    #[test]
    fn deleting_a_task_deletes_its_steps() {
        let mut t = Fixture::new();
        let task = t.task("task");
        let a = t.step("a", &task);
        t.step("b", &task);
        t.task("other");

        t.ok(delete(&a));
        assert_eq!(t.outline(), ["other", "task 0/1", "  b"]);
        t.ok(delete(&task));
        assert_eq!(t.outline(), ["other"]);
        assert_eq!((t.doc.task_count(), t.doc.order_len()), (1, 1));
        t.ok(Command::Undo);
        assert_eq!(t.outline(), ["other", "task 0/1", "  b"]);
    }

    #[test]
    fn delete_still_removes_a_malformed_task() {
        let mut t = Fixture::new();
        let task = t
            .doc
            .doc
            .get_map("tasks")
            .ensure_mergeable_map("x")
            .unwrap();
        task.insert("title", 42).unwrap();
        t.doc.doc.get_movable_list("order").insert(0, "x").unwrap();
        t.doc.doc.commit();
        t.ok(delete("x"));
        assert_eq!((t.doc.task_count(), t.doc.order_len()), (0, 0));
    }

    #[test]
    fn delete_and_add_report_what_they_cannot_find() {
        let mut t = Fixture::new();
        assert!(not_found(t.run(delete("gone"))));
        let add = Command::Add {
            title: "x".to_string(),
            after: None,
            due: None,
            list_id: Some("gone".to_string()),
            parent_id: None,
        };
        assert!(not_found(t.run(add)));
    }

    #[test]
    fn set_list_takes_steps_along_and_detaches_a_lone_step() {
        let mut t = Fixture::new();
        let work = t.work_list();
        t.task_in("there", &work);
        let task = t.task("task");
        let a = t.step("a", &task);
        t.step("b", &task);

        t.ok(set_list(&a, &work));
        assert_eq!(t.view_in(ViewFilter::All, &work), ["a", "there"]);
        t.ok(set_list(&task, &work));
        assert_eq!(
            t.view_in(ViewFilter::All, &work),
            ["task 0/1", "  b", "a", "there"]
        );
        assert!(t.outline().is_empty());
        t.ok(Command::Undo);
        assert_eq!(t.outline(), ["task 0/1", "  b"]);
    }

    #[test]
    fn set_list_clears_a_stored_parent_that_would_count_in_the_new_list() {
        let mut t = Fixture::new();
        let work = t.work_list();
        let target = t.task_in("target", &work);
        let stray = t.task("stray");
        t.raw(&stray, "parent_id", target.as_str());
        t.ok(set_list(&stray, &work));
        assert_eq!(t.view_in(ViewFilter::All, &work), ["stray", "target"]);
    }

    #[test]
    fn a_step_moves_only_among_its_siblings() {
        let mut t = Fixture::new();
        let other = t.task("other");
        let task = t.task("task");
        let a = t.step("a", &task);
        let b = t.step("b", &task);
        let c = t.step("c", &task);

        t.ok(mv(&a, Some(&c)));
        assert_eq!(t.outline(), ["task 0/3", "  b", "  c", "  a", "other"]);
        // `None` and the task itself both mean "first step".
        t.ok(mv(&a, None));
        assert_eq!(t.outline(), ["task 0/3", "  a", "  b", "  c", "other"]);
        t.ok(mv(&c, Some(&task)));
        assert_eq!(t.outline(), ["task 0/3", "  c", "  a", "  b", "other"]);

        let revision = t.revision();
        t.ok(mv(&c, None));
        assert_eq!(t.revision(), revision);
        assert!(rejected(t.run(mv(&b, Some(&other)))));
    }

    #[test]
    fn a_lone_step_moving_to_first_is_a_no_op() {
        let mut t = Fixture::new();
        let task = t.task("task");
        let only = t.step("only", &task);
        let revision = t.revision();
        t.ok(mv(&only, None));
        assert_eq!(t.revision(), revision);
    }

    #[test]
    fn a_task_moved_after_a_step_lands_below_that_steps_block() {
        let mut t = Fixture::new();
        let x = t.task("x");
        let task = t.task("task");
        t.step("a", &task);
        let b = t.step("b", &task);
        let top = t.task("top");
        assert_eq!(t.outline(), ["top", "task 0/2", "  a", "  b", "x"]);

        t.ok(mv(&top, Some(&b)));
        assert_eq!(t.outline(), ["task 0/2", "  a", "  b", "top", "x"]);
        t.ok(mv(&x, Some(&task)));
        assert_eq!(t.outline(), ["task 0/2", "  a", "  b", "x", "top"]);
    }

    #[test]
    fn relative_commands_are_not_doc_commands() {
        let mut t = Fixture::new();
        let id = "a".to_string();
        assert!(rejected(t.run(Command::Indent { id: id.clone() })));
        assert!(rejected(t.run(Command::Outdent { id: id.clone() })));
        assert!(rejected(t.run(Command::Shift { id, delta: 1 })));
    }
}
