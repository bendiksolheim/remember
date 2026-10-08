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
            } => Command::Add {
                title,
                after,
                due,
                list_id: Some(capture_list_id.to_string()),
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
}
