//! The in-memory half of [`crate::App`]: a document plus "what is this
//! client looking at" (view filter, current list), and the business rules
//! that tie the two together. No I/O, no threads — `App` adds SQLite
//! persistence and debounced writes on top, while a host with its own
//! storage (the web build) can drive a `Session` directly.

use crate::clock::{Clock, IdSource};
use crate::command::{Command, ViewFilter};
use crate::doc::{self, Doc};
use crate::snapshot::{self, DueDetection, Snapshot};
use crate::{civil, due_parse, CoreError};

pub struct Session {
    doc: Doc,
    view: ViewFilter,
    /// The list `current()` reads, and the one new captures land in —
    /// always a concrete list id.
    current_list: String,
}

impl Session {
    /// `current_list: None` starts on the default list.
    pub fn new(doc: Doc, current_list: Option<String>) -> Self {
        Self {
            doc,
            view: ViewFilter::default(),
            current_list: current_list.unwrap_or_else(|| doc::DEFAULT_LIST_ID.to_string()),
        }
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn doc_mut(&mut self) -> &mut Doc {
        &mut self.doc
    }

    /// Swaps in `doc` and points back at the default list, keeping the
    /// view and the local UTC offset.
    pub fn reset(&mut self, mut doc: Doc) {
        doc.set_local_offset_seconds(self.doc.local_offset_seconds());
        self.doc = doc;
        self.current_list = doc::DEFAULT_LIST_ID.to_string();
    }

    /// Applies `command`, filling in `Command::Add`'s missing `list_id`
    /// with the current list. Returns `true` when this also moved
    /// `current_list` (because `command` deleted the list it pointed at),
    /// so a caller that persists the current list knows to save it.
    pub fn dispatch(
        &mut self,
        command: Command,
        clock: &dyn Clock,
        ids: &dyn IdSource,
    ) -> Result<bool, CoreError> {
        let Some(command) = self.resolve_relative(command, clock)? else {
            return Ok(false);
        };
        let command = Self::resolve_capture_list(command, &self.current_list);
        let deleted_list_id = match &command {
            Command::DeleteList { id } => Some(id.clone()),
            _ => None,
        };
        self.doc.apply(command, clock, ids)?;
        match deleted_list_id {
            Some(deleted_id) => self.repair_list_pointers(&deleted_id, clock),
            None => Ok(false),
        }
    }

    /// Re-points `current_list` when a successful `DeleteList` just removed
    /// the list it was pointing at — otherwise the next capture would fail
    /// with `NotFound` (or the view would keep filtering on a list that no
    /// longer exists) until the user manually picked a new one. Falls back
    /// to the first list in sidebar order; `apply_delete_list`'s
    /// last-remaining-list guard means there's always at least one left.
    fn repair_list_pointers(
        &mut self,
        deleted_id: &str,
        clock: &dyn Clock,
    ) -> Result<bool, CoreError> {
        if self.current_list != deleted_id {
            return Ok(false);
        }

        let snapshot = self.doc.read(ViewFilter::All, deleted_id, clock);
        let fallback_id =
            snapshot
                .lists
                .first()
                .map(|list| list.id.clone())
                .ok_or(CoreError::Document(
                    "no lists remain after delete".to_string(),
                ))?;

        self.current_list = fallback_id;
        Ok(true)
    }

    /// Turns `Indent`, `Outdent` and `Shift` into explicit `Doc` commands
    /// against the rows currently shown. `None` when there's nothing to do;
    /// other commands pass through.
    fn resolve_relative(
        &self,
        command: Command,
        clock: &dyn Clock,
    ) -> Result<Option<Command>, CoreError> {
        enum Relative {
            Indent,
            Outdent,
            Shift(i32),
        }
        let (id, relative) = match command {
            Command::Indent { id } => (id, Relative::Indent),
            Command::Outdent { id } => (id, Relative::Outdent),
            Command::Shift { id, delta } => (id, Relative::Shift(delta)),
            other => return Ok(Some(other)),
        };
        let rows = self.current(clock).rows;
        let index = rows
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| CoreError::NotFound(id.clone()))?;
        let row = &rows[index];

        Ok(match relative {
            Relative::Indent => {
                if row.parent_id.is_some() || row.step_count > 0 {
                    return Ok(None);
                }
                rows[..index]
                    .iter()
                    .rev()
                    .find(|r| r.parent_id.is_none())
                    .map(|above| Command::SetParent {
                        id,
                        parent_id: Some(above.id.clone()),
                    })
            }
            Relative::Outdent => row.parent_id.as_ref().map(|_| Command::SetParent {
                id,
                parent_id: None,
            }),
            Relative::Shift(delta) => {
                // A step's peers are its siblings; a task's are the
                // top-level rows, each standing for its block.
                let peers: Vec<&str> = rows
                    .iter()
                    .filter(|r| r.parent_id == row.parent_id)
                    .map(|r| r.id.as_str())
                    .collect();
                let from = peers.iter().position(|&p| p == id).unwrap_or_default();
                let last = peers.len() as i64 - 1;
                let to = (from as i64 + i64::from(delta)).clamp(0, last) as usize;
                let after = match to.cmp(&from) {
                    std::cmp::Ordering::Equal => return Ok(None),
                    std::cmp::Ordering::Greater => Some(peers[to].to_string()),
                    // After the task itself: `Doc`'s "first step".
                    std::cmp::Ordering::Less if to > 0 => Some(peers[to - 1].to_string()),
                    std::cmp::Ordering::Less => row.parent_id.clone(),
                };
                Some(Command::Move { id, after })
            }
        })
    }

    /// Fills in `Command::Add`'s `list_id` with the sticky current capture
    /// list when the caller didn't specify one — the one piece of "which
    /// list is this app currently pointed at" business logic, kept here so
    /// every caller gets it for free rather than reimplementing
    /// it per platform.
    fn resolve_capture_list(command: Command, capture_list_id: &str) -> Command {
        match command {
            Command::Add {
                title,
                after,
                due,
                list_id: None,
                parent_id,
            } => Command::Add {
                title,
                after,
                due,
                list_id: Some(capture_list_id.to_string()),
                parent_id,
            },
            other => other,
        }
    }

    pub fn set_view(&mut self, view: ViewFilter) {
        self.view = view;
    }

    pub fn current_list(&self) -> &str {
        &self.current_list
    }

    pub fn set_current_list(&mut self, list_id: String) {
        self.current_list = list_id;
    }

    /// See [`crate::App::set_local_offset_seconds`].
    pub fn set_local_offset_seconds(&mut self, offset_seconds: i32) {
        self.doc.set_local_offset_seconds(offset_seconds);
    }

    /// Detects a due-date phrase at the end of `text` (see [`due_parse`]),
    /// resolved against the current local day — the same "today" `current()`
    /// uses for `due_label`/`due_state`. A pure lookup: never touches the
    /// document, never commits, never bumps `revision`. `None` if `text`
    /// doesn't end in a recognized phrase.
    pub fn detect_due(&self, text: &str, clock: &dyn Clock) -> Option<DueDetection> {
        let now = clock.now();
        let offset = self.doc.local_offset_seconds();
        let today = civil::day_number(now + i64::from(offset));
        let detection = due_parse::detect(text, today)?;
        let due = detection.day_number * civil::SECONDS_PER_DAY - i64::from(offset);
        Some(DueDetection {
            stripped_title: text[..detection.match_start].trim_end().to_string(),
            label: snapshot::due_label(due, now, offset),
            due,
        })
    }

    pub fn current(&self, clock: &dyn Clock) -> Snapshot {
        self.doc.read(self.view, &self.current_list, clock)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::clock::{FixedClock, SeqIdSource};

    const NOW: i64 = 1_760_000_000;

    fn session() -> Session {
        Session::new(Doc::new(1).expect("new doc"), None)
    }

    fn add(title: &str) -> Command {
        Command::Add {
            title: title.into(),
            after: None,
            due: None,
            list_id: None,
            parent_id: None,
        }
    }

    fn add_list(s: &mut Session, ids: &SeqIdSource, name: &str) -> String {
        s.dispatch(
            Command::AddList {
                name: name.into(),
                after: None,
            },
            &FixedClock(NOW),
            ids,
        )
        .expect("add list");
        s.current(&FixedClock(NOW))
            .lists
            .into_iter()
            .find(|l| l.name == name)
            .expect("list exists")
            .id
    }

    #[test]
    fn starts_on_default_list() {
        assert_eq!(session().current_list(), doc::DEFAULT_LIST_ID);
    }

    #[test]
    fn add_without_list_lands_in_current_list() {
        let (clock, ids) = (FixedClock(NOW), SeqIdSource::new());
        let mut s = session();
        let work = add_list(&mut s, &ids, "Work");
        s.set_current_list(work.clone());

        assert!(!s.dispatch(add("Ship it"), &clock, &ids).expect("add"));

        let rows = s.current(&clock).rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].list_id, work);
    }

    #[test]
    fn deleting_current_list_moves_to_first_remaining() {
        let (clock, ids) = (FixedClock(NOW), SeqIdSource::new());
        let mut s = session();
        let work = add_list(&mut s, &ids, "Work");
        s.set_current_list(work.clone());

        assert!(s
            .dispatch(Command::DeleteList { id: work }, &clock, &ids)
            .expect("delete"));
        assert_eq!(s.current_list(), doc::DEFAULT_LIST_ID);
    }

    #[test]
    fn deleting_another_list_keeps_current() {
        let (clock, ids) = (FixedClock(NOW), SeqIdSource::new());
        let mut s = session();
        let work = add_list(&mut s, &ids, "Work");

        assert!(!s
            .dispatch(Command::DeleteList { id: work }, &clock, &ids)
            .expect("delete"));
        assert_eq!(s.current_list(), doc::DEFAULT_LIST_ID);
    }

    #[test]
    fn reset_keeps_view_and_offset_but_returns_to_default_list() {
        let (clock, ids) = (FixedClock(NOW), SeqIdSource::new());
        let mut s = session();
        let work = add_list(&mut s, &ids, "Work");
        s.set_current_list(work);
        s.set_view(ViewFilter::Active);
        s.set_local_offset_seconds(3600);
        s.dispatch(add("old"), &clock, &ids).expect("add");

        s.reset(Doc::new(2).expect("new doc"));

        let snapshot = s.current(&clock);
        assert!(snapshot.rows.is_empty());
        assert_eq!(snapshot.view, ViewFilter::Active);
        assert_eq!(s.current_list(), doc::DEFAULT_LIST_ID);
        assert_eq!(s.doc().local_offset_seconds(), 3600);
    }

    #[test]
    fn detect_due_strips_phrase() {
        let detection = session()
            .detect_due("call mum tomorrow", &FixedClock(NOW))
            .expect("detected");
        assert_eq!(detection.stripped_title, "call mum");
        assert_eq!(detection.label, "Tomorrow");
    }

    /// Adds top-level tasks in display order; returns their ids.
    fn add_tasks(s: &mut Session, ids: &SeqIdSource, titles: &[&str]) -> Vec<String> {
        for title in titles.iter().rev() {
            s.dispatch(add(title), &FixedClock(NOW), ids).expect("add");
        }
        let rows = s.current(&FixedClock(NOW)).rows;
        titles
            .iter()
            .map(|t| {
                rows.iter()
                    .find(|r| r.title == *t)
                    .expect("added")
                    .id
                    .clone()
            })
            .collect()
    }

    fn outline(s: &Session) -> Vec<String> {
        s.current(&FixedClock(NOW))
            .rows
            .into_iter()
            .map(|r| match r.parent_id {
                Some(_) => format!("  {}", r.title),
                None => r.title,
            })
            .collect()
    }

    fn run(s: &mut Session, ids: &SeqIdSource, command: Command) {
        s.dispatch(command, &FixedClock(NOW), ids)
            .expect("dispatch");
    }

    fn revision(s: &Session) -> u64 {
        s.current(&FixedClock(NOW)).revision
    }

    fn indent(id: &str) -> Command {
        Command::Indent { id: id.into() }
    }

    fn shift(id: &str, delta: i32) -> Command {
        Command::Shift {
            id: id.into(),
            delta,
        }
    }

    #[test]
    fn indent_tucks_a_task_under_the_nearest_visible_task_above() {
        let mut s = session();
        let ids = SeqIdSource::new();
        let t = add_tasks(&mut s, &ids, &["a", "b", "hidden", "c"]);
        let done = Command::SetDone {
            id: t[2].clone(),
            done: true,
        };
        run(&mut s, &ids, done);
        s.set_view(ViewFilter::Active);

        run(&mut s, &ids, indent(&t[1]));
        // c skips both the hidden done task and the step b.
        run(&mut s, &ids, indent(&t[3]));
        assert_eq!(outline(&s), ["a", "  b", "  c"]);

        let before = revision(&s);
        // The first row, a step, and a task with steps all stay put.
        run(&mut s, &ids, indent(&t[0]));
        run(&mut s, &ids, indent(&t[1]));
        let top = add_tasks(&mut s, &ids, &["top"]);
        run(&mut s, &ids, indent(&t[0]));
        assert_eq!(revision(&s), before + 1); // just the add
        assert_eq!(outline(&s), ["top", "a", "  b", "  c"]);
        run(&mut s, &ids, indent(&top[0]));
        assert_eq!(revision(&s), before + 1);

        let hidden = s.dispatch(indent(&t[2]), &FixedClock(NOW), &ids);
        assert!(matches!(hidden, Err(CoreError::NotFound(_))));
    }

    #[test]
    fn outdent_turns_a_step_back_into_a_task() {
        let mut s = session();
        let ids = SeqIdSource::new();
        let t = add_tasks(&mut s, &ids, &["a", "b", "c"]);
        run(&mut s, &ids, indent(&t[1]));
        assert_eq!(outline(&s), ["a", "  b", "c"]);
        run(&mut s, &ids, Command::Outdent { id: t[1].clone() });
        assert_eq!(outline(&s), ["a", "b", "c"]);
        let before = revision(&s);
        run(&mut s, &ids, Command::Outdent { id: t[1].clone() });
        assert_eq!(revision(&s), before);
    }

    #[test]
    fn shift_moves_whole_tasks_and_steps_among_siblings() {
        let mut s = session();
        let ids = SeqIdSource::new();
        let t = add_tasks(&mut s, &ids, &["a", "s1", "s2", "b", "c"]);
        let (a, s1, s2, b, c) = (&t[0], &t[1], &t[2], &t[3], &t[4]);
        run(&mut s, &ids, indent(s1));
        run(&mut s, &ids, indent(s2));

        run(&mut s, &ids, shift(b, -1));
        assert_eq!(outline(&s), ["b", "a", "  s1", "  s2", "c"]);
        run(&mut s, &ids, shift(b, 1));
        assert_eq!(outline(&s), ["a", "  s1", "  s2", "b", "c"]);
        run(&mut s, &ids, shift(a, 1));
        assert_eq!(outline(&s), ["b", "a", "  s1", "  s2", "c"]);
        run(&mut s, &ids, shift(a, -5));
        assert_eq!(outline(&s), ["a", "  s1", "  s2", "b", "c"]);

        run(&mut s, &ids, shift(s1, 1));
        assert_eq!(outline(&s), ["a", "  s2", "  s1", "b", "c"]);
        run(&mut s, &ids, shift(s1, -1));
        assert_eq!(outline(&s), ["a", "  s1", "  s2", "b", "c"]);

        let before = revision(&s);
        run(&mut s, &ids, shift(c, 5));
        run(&mut s, &ids, shift(s1, -1));
        run(&mut s, &ids, shift(s2, 3));
        assert_eq!(revision(&s), before);
    }
}
