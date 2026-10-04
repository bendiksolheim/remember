use todo_core::{Command, CoreError, Doc, DueState, FixedClock, SeqIdSource, ViewFilter};

fn add(doc: &mut Doc, clock: &FixedClock, ids: &SeqIdSource, title: &str) -> String {
    doc.apply(
        Command::Add {
            title: title.to_string(),
            after: None,
            due: None,
            list_id: None,
        },
        clock,
        ids,
    )
    .unwrap();
    doc.read(ViewFilter::All, "default", clock).rows[0]
        .id
        .clone()
}

fn titles(snap: &todo_core::Snapshot) -> Vec<String> {
    snap.rows.iter().map(|r| r.title.clone()).collect()
}

fn ids_of(snap: &todo_core::Snapshot) -> Vec<String> {
    snap.rows.iter().map(|r| r.id.clone()).collect()
}

#[test]
fn add_three_tasks_order_reflects_insertion() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add(&mut doc, &clock, &ids, "a");
    add(&mut doc, &clock, &ids, "b");
    add(&mut doc, &clock, &ids, "c");
    // Each Add uses after:None ("insert at the top"), so order is LIFO.
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(titles(&snap), vec!["c", "b", "a"]);
}

#[test]
fn add_after_none_inserts_at_top_after_some_inserts_after() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    doc.apply(
        Command::Add {
            title: "b".to_string(),
            after: Some(a),
            due: None,
            list_id: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        titles(&doc.read(ViewFilter::All, "default", &clock)),
        vec!["a", "b"]
    );

    add(&mut doc, &clock, &ids, "c");
    assert_eq!(
        titles(&doc.read(ViewFilter::All, "default", &clock)),
        vec!["c", "a", "b"]
    );
}

#[test]
fn add_after_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::Add {
                title: "a".to_string(),
                after: Some("nope".to_string()),
                due: None,
                list_id: None,
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn set_title_updates_only_title() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(
        Command::SetNotes {
            id: id.clone(),
            notes: "note".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    doc.apply(
        Command::SetTitle {
            id: id.clone(),
            title: "new".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let row = &doc.read(ViewFilter::All, "default", &clock).rows[0];
    assert_eq!(row.title, "new");
    assert_eq!(row.notes, "note");
    assert!(!row.done);
}

#[test]
fn set_title_to_current_value_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    let before = doc.read(ViewFilter::All, "default", &clock).revision;
    doc.apply(
        Command::SetTitle {
            id,
            title: "a".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        doc.read(ViewFilter::All, "default", &clock).revision,
        before
    );
}

#[test]
fn set_title_trims_surrounding_whitespace() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(
        Command::SetTitle {
            id,
            title: "  new title  ".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        titles(&doc.read(ViewFilter::All, "default", &clock)),
        vec!["new title"]
    );
}

#[test]
fn set_title_empty_after_trim_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    let before = doc.read(ViewFilter::All, "default", &clock).revision;
    doc.apply(
        Command::SetTitle {
            id,
            title: "   ".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(snap.revision, before);
    assert_eq!(titles(&snap), vec!["a"]);
}

#[test]
fn set_title_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::SetTitle {
                id: "nope".to_string(),
                title: "new".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn set_title_unknown_id_with_empty_title_is_still_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::SetTitle {
                id: "nope".to_string(),
                title: "   ".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn add_trims_surrounding_whitespace() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add(&mut doc, &clock, &ids, "  a  ");
    assert_eq!(
        titles(&doc.read(ViewFilter::All, "default", &clock)),
        vec!["a"]
    );
}

#[test]
fn add_empty_after_trim_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let before = doc.read(ViewFilter::All, "default", &clock).revision;
    doc.apply(
        Command::Add {
            title: "   ".to_string(),
            after: None,
            due: None,
            list_id: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(snap.revision, before);
    assert!(snap.rows.is_empty());
}

#[test]
fn set_done_to_current_value_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    let before = doc.read(ViewFilter::All, "default", &clock).revision;
    doc.apply(Command::SetDone { id, done: false }, &clock, &ids)
        .unwrap();
    assert_eq!(
        doc.read(ViewFilter::All, "default", &clock).revision,
        before
    );
}

#[test]
fn set_notes_updates_only_notes() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(
        Command::SetNotes {
            id: id.clone(),
            notes: "hello".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let row = &doc.read(ViewFilter::All, "default", &clock).rows[0];
    assert_eq!(row.title, "a");
    assert_eq!(row.notes, "hello");
}

#[test]
fn set_due_updates_only_due() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(
        Command::SetDue {
            id: id.clone(),
            due: Some(1_000),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let row = &doc.read(ViewFilter::All, "default", &clock).rows[0];
    assert_eq!(row.title, "a");
    assert_eq!(row.due, Some(1_000));
}

#[test]
fn set_due_none_clears_date_and_due_state() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(10 * 86_400);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(
        Command::SetDue {
            id: id.clone(),
            due: Some(0),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let row = doc.read(ViewFilter::All, "default", &clock).rows[0].clone();
    assert_eq!(row.due_state, DueState::Overdue);

    doc.apply(
        Command::SetDue {
            id: id.clone(),
            due: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let row = &doc.read(ViewFilter::All, "default", &clock).rows[0];
    assert_eq!(row.due, None);
    assert_eq!(row.due_label, None);
    assert_eq!(row.due_state, DueState::None);
}

#[test]
fn move_to_first_last_middle_and_own_position_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    let b = add(&mut doc, &clock, &ids, "b");
    let c = add(&mut doc, &clock, &ids, "c");
    let d = add(&mut doc, &clock, &ids, "d");
    // after:None each time -> initial order: [d, c, b, a]
    assert_eq!(
        ids_of(&doc.read(ViewFilter::All, "default", &clock)),
        vec![d.clone(), c.clone(), b.clone(), a.clone()]
    );

    // Move 'a' to first.
    doc.apply(
        Command::Move {
            id: a.clone(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        ids_of(&doc.read(ViewFilter::All, "default", &clock)),
        vec![a.clone(), d.clone(), c.clone(), b.clone()]
    );

    // Move 'd' to last (after the current last, 'b').
    doc.apply(
        Command::Move {
            id: d.clone(),
            after: Some(b.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        ids_of(&doc.read(ViewFilter::All, "default", &clock)),
        vec![a.clone(), c.clone(), b.clone(), d.clone()]
    );

    // Move 'b' to the middle (after 'a').
    doc.apply(
        Command::Move {
            id: b.clone(),
            after: Some(a.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        ids_of(&doc.read(ViewFilter::All, "default", &clock)),
        vec![a.clone(), b.clone(), c.clone(), d.clone()]
    );

    // Move 'b' after 'a' again: it's already there, so this is a no-op.
    let before = ids_of(&doc.read(ViewFilter::All, "default", &clock));
    doc.apply(
        Command::Move {
            id: b.clone(),
            after: Some(a.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(
        ids_of(&doc.read(ViewFilter::All, "default", &clock)),
        before
    );
}

#[test]
fn move_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add(&mut doc, &clock, &ids, "a");
    let err = doc
        .apply(
            Command::Move {
                id: "nope".to_string(),
                after: None,
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn move_unknown_after_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    let err = doc
        .apply(
            Command::Move {
                id: a,
                after: Some("nope".to_string()),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn delete_removes_from_both_maps() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    add(&mut doc, &clock, &ids, "b");
    doc.apply(Command::Delete { id: a.clone() }, &clock, &ids)
        .unwrap();
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(titles(&snap), vec!["b"]);
    assert!(snap.rows.iter().all(|r| r.id != a));
}

#[test]
fn delete_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::Delete {
                id: "nope".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn set_done_hides_from_active_view_but_not_count() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    add(&mut doc, &clock, &ids, "b");
    assert_eq!(doc.read(ViewFilter::All, "default", &clock).active_count, 2);

    doc.apply(
        Command::SetDone {
            id: a.clone(),
            done: true,
        },
        &clock,
        &ids,
    )
    .unwrap();

    let active = doc.read(ViewFilter::Active, "default", &clock);
    assert_eq!(active.active_count, 1);
    assert!(active.rows.iter().all(|r| r.id != a));
}

#[test]
fn completed_view_shows_only_done_tasks() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    add(&mut doc, &clock, &ids, "b");
    doc.apply(
        Command::SetDone {
            id: a.clone(),
            done: true,
        },
        &clock,
        &ids,
    )
    .unwrap();

    let completed = doc.read(ViewFilter::Completed, "default", &clock);
    assert_eq!(titles(&completed), vec!["a"]);
    assert!(completed.rows[0].done);
}

#[test]
fn undo_restores_prior_snapshot_after_each_mutating_command() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();

    let empty = ids_of(&doc.read(ViewFilter::All, "default", &clock));
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(Command::Undo, &clock, &ids).unwrap();
    assert_eq!(ids_of(&doc.read(ViewFilter::All, "default", &clock)), empty);

    doc.apply(Command::Redo, &clock, &ids).unwrap();
    let after_add = doc.read(ViewFilter::All, "default", &clock).rows;

    doc.apply(
        Command::SetTitle {
            id: id.clone(),
            title: "changed".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    doc.apply(Command::Undo, &clock, &ids).unwrap();
    assert_eq!(doc.read(ViewFilter::All, "default", &clock).rows, after_add);
}

#[test]
fn redo_after_undo_restores_post_command_snapshot() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add(&mut doc, &clock, &ids, "a");
    let after_add = doc.read(ViewFilter::All, "default", &clock).rows;

    doc.apply(Command::Undo, &clock, &ids).unwrap();
    assert!(doc.read(ViewFilter::All, "default", &clock).rows.is_empty());

    doc.apply(Command::Redo, &clock, &ids).unwrap();
    assert_eq!(doc.read(ViewFilter::All, "default", &clock).rows, after_add);
}

#[test]
fn undo_on_empty_history_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let before = doc.read(ViewFilter::All, "default", &clock);
    assert!(!before.can_undo);

    doc.apply(Command::Undo, &clock, &ids).unwrap();

    let after = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.rows, before.rows);
}

#[test]
fn revision_strictly_increases_across_every_mutation() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let mut last = doc.read(ViewFilter::All, "default", &clock).revision;

    let mut bump = |doc: &mut Doc, cmd: Command| {
        doc.apply(cmd, &clock, &ids).unwrap();
        let rev = doc.read(ViewFilter::All, "default", &clock).revision;
        assert!(rev > last, "revision did not increase: {last} -> {rev}");
        last = rev;
    };

    bump(
        &mut doc,
        Command::Add {
            title: "a".to_string(),
            after: None,
            due: None,
            list_id: None,
        },
    );
    let id = doc.read(ViewFilter::All, "default", &clock).rows[0]
        .id
        .clone();
    bump(
        &mut doc,
        Command::SetTitle {
            id: id.clone(),
            title: "b".to_string(),
        },
    );
    bump(
        &mut doc,
        Command::Add {
            title: "c".to_string(),
            after: None,
            due: None,
            list_id: None,
        },
    );
    // A real (non-no-op) move: `id` is currently second, this brings it to
    // the front — a mutated `revision += 1` (e.g. `-=`/`*=`) wouldn't be
    // caught by commands that don't take this specific code path.
    bump(
        &mut doc,
        Command::Move {
            id: id.clone(),
            after: None,
        },
    );
    bump(&mut doc, Command::Undo);
    bump(&mut doc, Command::Redo);
    bump(&mut doc, Command::Delete { id });
}

#[test]
fn empty_document_has_no_rows() {
    let doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert!(snap.rows.is_empty());
    assert_eq!(snap.active_count, 0);
}

fn add_list(doc: &mut Doc, clock: &FixedClock, ids: &SeqIdSource, name: &str) -> String {
    doc.apply(
        Command::AddList {
            name: name.to_string(),
            after: None,
        },
        clock,
        ids,
    )
    .unwrap();
    doc.read(ViewFilter::All, "default", clock).lists[0]
        .id
        .clone()
}

#[test]
fn fresh_doc_has_one_default_list_named_tasks() {
    let doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(snap.lists.len(), 1);
    assert_eq!(snap.lists[0].name, "Tasks");
}

#[test]
fn add_lands_new_task_in_the_default_list() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add(&mut doc, &clock, &ids, "a");
    let row = &doc.read(ViewFilter::All, "default", &clock).rows[0];
    assert_eq!(row.list_id, "default");
    assert_eq!(row.list_name, "Tasks");
}

#[test]
fn add_with_explicit_list_id_stores_it_and_resolves_its_name() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");

    doc.apply(
        Command::Add {
            title: "a".to_string(),
            after: None,
            due: None,
            list_id: Some(work.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();

    let row = &doc.read(ViewFilter::All, &work, &clock).rows[0];
    assert_eq!(row.list_id, work);
    assert_eq!(row.list_name, "Work");
}

#[test]
fn add_with_unknown_list_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::Add {
                title: "a".to_string(),
                after: None,
                due: None,
                list_id: Some("nope".to_string()),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn add_list_after_none_inserts_at_front() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add_list(&mut doc, &clock, &ids, "Work");
    let names: Vec<String> = doc
        .read(ViewFilter::All, "default", &clock)
        .lists
        .iter()
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(names, vec!["Work", "Tasks"]);
}

#[test]
fn add_list_after_some_inserts_after_that_list() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let default_id = doc.read(ViewFilter::All, "default", &clock).lists[0]
        .id
        .clone();
    doc.apply(
        Command::AddList {
            name: "Work".to_string(),
            after: Some(default_id),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let names: Vec<String> = doc
        .read(ViewFilter::All, "default", &clock)
        .lists
        .iter()
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(names, vec!["Tasks", "Work"]);
}

#[test]
fn add_list_empty_name_after_trim_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let before = doc.read(ViewFilter::All, "default", &clock);
    doc.apply(
        Command::AddList {
            name: "   ".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let after = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(after.lists.len(), before.lists.len());
    assert_eq!(after.revision, before.revision);
}

#[test]
fn add_list_after_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::AddList {
                name: "Work".to_string(),
                after: Some("nope".to_string()),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn rename_list_updates_name() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");
    doc.apply(
        Command::RenameList {
            id: work.clone(),
            name: "Office".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(
        snap.lists.iter().find(|l| l.id == work).unwrap().name,
        "Office"
    );
}

#[test]
fn rename_list_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::RenameList {
                id: "nope".to_string(),
                name: "Office".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn rename_list_empty_name_after_trim_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");
    let before = doc.read(ViewFilter::All, "default", &clock);
    doc.apply(
        Command::RenameList {
            id: work,
            name: "   ".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let after = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(after.revision, before.revision);
}

#[test]
fn rename_list_to_its_current_name_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");
    let before = doc.read(ViewFilter::All, "default", &clock);
    doc.apply(
        Command::RenameList {
            id: work,
            name: "Work".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let after = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(after.revision, before.revision);
}

#[test]
fn delete_list_cascades_its_tasks_but_leaves_other_lists_alone() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");
    add(&mut doc, &clock, &ids, "default task");
    doc.apply(
        Command::Add {
            title: "work task".to_string(),
            after: None,
            due: None,
            list_id: Some(work.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc.apply(Command::DeleteList { id: work.clone() }, &clock, &ids)
        .unwrap();

    let snap = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(titles(&snap), vec!["default task"]);
    assert!(!snap.lists.iter().any(|l| l.id == work));
}

#[test]
fn delete_list_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::DeleteList {
                id: "nope".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn delete_last_remaining_list_is_rejected() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let default_id = doc.read(ViewFilter::All, "default", &clock).lists[0]
        .id
        .clone();
    let err = doc
        .apply(Command::DeleteList { id: default_id }, &clock, &ids)
        .unwrap_err();
    assert!(matches!(err, CoreError::Document(_)));
}

#[test]
fn move_list_reorders() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work"); // [Work, Tasks]
    let personal = add_list(&mut doc, &clock, &ids, "Personal"); // [Personal, Work, Tasks]

    doc.apply(
        Command::MoveList {
            id: personal,
            after: Some(work),
        },
        &clock,
        &ids,
    )
    .unwrap();

    let names: Vec<String> = doc
        .read(ViewFilter::All, "default", &clock)
        .lists
        .iter()
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(names, vec!["Work", "Personal", "Tasks"]);
}

#[test]
fn move_list_unknown_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::MoveList {
                id: "nope".to_string(),
                after: None,
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn move_list_to_its_own_position_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let default_id = doc.read(ViewFilter::All, "default", &clock).lists[0]
        .id
        .clone();
    let before = doc.read(ViewFilter::All, "default", &clock);
    doc.apply(
        Command::MoveList {
            id: default_id,
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let after = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(after.revision, before.revision);
}

#[test]
fn set_list_moves_task_to_the_top_of_its_new_list() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");
    let a = add(&mut doc, &clock, &ids, "a");
    add(&mut doc, &clock, &ids, "b"); // now [b, a], both in default

    doc.apply(
        Command::SetList {
            id: a.clone(),
            list_id: work.clone(),
        },
        &clock,
        &ids,
    )
    .unwrap();

    let snap = doc.read(ViewFilter::All, &work, &clock);
    assert_eq!(snap.rows[0].id, a);
    assert_eq!(snap.rows[0].list_id, work);
}

#[test]
fn set_list_to_its_current_list_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    let before = doc.read(ViewFilter::All, "default", &clock);
    doc.apply(
        Command::SetList {
            id: a,
            list_id: "default".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let after = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(after.revision, before.revision);
}

#[test]
fn set_list_unknown_task_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let err = doc
        .apply(
            Command::SetList {
                id: "nope".to_string(),
                list_id: "default".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn set_list_unknown_list_id_is_not_found() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let a = add(&mut doc, &clock, &ids, "a");
    let err = doc
        .apply(
            Command::SetList {
                id: a,
                list_id: "nope".to_string(),
            },
            &clock,
            &ids,
        )
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("nope".to_string()));
}

#[test]
fn read_filters_rows_by_list_while_still_resolving_every_lists_name() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let work = add_list(&mut doc, &clock, &ids, "Work");
    add(&mut doc, &clock, &ids, "default task");
    doc.apply(
        Command::Add {
            title: "work task".to_string(),
            after: None,
            due: None,
            list_id: Some(work.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();

    let work_only = doc.read(ViewFilter::All, &work, &clock);
    assert_eq!(titles(&work_only), vec!["work task"]);
    // Even scoped to one list, the full roster is still there (for a list
    // picker UI) — `lists` isn't itself filtered by `current_list`.
    assert_eq!(work_only.lists.len(), 2);

    let default_only = doc.read(ViewFilter::All, "default", &clock);
    assert_eq!(titles(&default_only), vec!["default task"]);
}

#[test]
fn list_command_revisions_all_increase_on_real_change() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let mut last = doc.read(ViewFilter::All, "default", &clock).revision;

    let mut bump = |doc: &mut Doc, cmd: Command| {
        doc.apply(cmd, &clock, &ids).unwrap();
        let rev = doc.read(ViewFilter::All, "default", &clock).revision;
        assert!(rev > last, "revision did not increase: {last} -> {rev}");
        last = rev;
    };

    bump(
        &mut doc,
        Command::AddList {
            name: "Work".to_string(),
            after: None,
        },
    );
    let work = doc.read(ViewFilter::All, "default", &clock).lists[0]
        .id
        .clone();
    bump(
        &mut doc,
        Command::RenameList {
            id: work.clone(),
            name: "Office".to_string(),
        },
    );
    bump(
        &mut doc,
        Command::Add {
            title: "a".to_string(),
            after: None,
            due: None,
            list_id: None,
        },
    );
    let a = doc.read(ViewFilter::All, "default", &clock).rows[0]
        .id
        .clone();
    bump(
        &mut doc,
        Command::SetList {
            id: a,
            list_id: work.clone(),
        },
    );
    // AddList(after: None) put "Office" at the front, so moving it to the
    // back (after the default list) is a real change, not a no-op.
    let default_id = doc
        .read(ViewFilter::All, "default", &clock)
        .lists
        .iter()
        .find(|l| l.id != work)
        .unwrap()
        .id
        .clone();
    bump(
        &mut doc,
        Command::MoveList {
            id: work.clone(),
            after: Some(default_id),
        },
    );
    bump(&mut doc, Command::DeleteList { id: work });
}
