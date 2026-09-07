use todo_core::{Command, CoreError, Doc, FixedClock, SeqIdSource, ViewFilter};

fn add(doc: &mut Doc, clock: &FixedClock, ids: &SeqIdSource, title: &str) -> String {
    doc.apply(
        Command::Add {
            title: title.to_string(),
            after: None,
        },
        clock,
        ids,
    )
    .unwrap();
    doc.read(ViewFilter::All, clock).rows[0].id.clone()
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
    let snap = doc.read(ViewFilter::All, &clock);
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
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(titles(&doc.read(ViewFilter::All, &clock)), vec!["a", "b"]);

    add(&mut doc, &clock, &ids, "c");
    assert_eq!(
        titles(&doc.read(ViewFilter::All, &clock)),
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
    let row = &doc.read(ViewFilter::All, &clock).rows[0];
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
    let before = doc.read(ViewFilter::All, &clock).revision;
    doc.apply(
        Command::SetTitle {
            id,
            title: "a".to_string(),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(doc.read(ViewFilter::All, &clock).revision, before);
}

#[test]
fn set_done_to_current_value_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let id = add(&mut doc, &clock, &ids, "a");
    let before = doc.read(ViewFilter::All, &clock).revision;
    doc.apply(Command::SetDone { id, done: false }, &clock, &ids)
        .unwrap();
    assert_eq!(doc.read(ViewFilter::All, &clock).revision, before);
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
    let row = &doc.read(ViewFilter::All, &clock).rows[0];
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
    let row = &doc.read(ViewFilter::All, &clock).rows[0];
    assert_eq!(row.title, "a");
    assert_eq!(row.due, Some(1_000));
}

#[test]
fn set_due_none_clears_date_and_overdue() {
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
    let row = doc.read(ViewFilter::All, &clock).rows[0].clone();
    assert!(row.overdue);

    doc.apply(
        Command::SetDue {
            id: id.clone(),
            due: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let row = &doc.read(ViewFilter::All, &clock).rows[0];
    assert_eq!(row.due, None);
    assert_eq!(row.due_label, None);
    assert!(!row.overdue);
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
        ids_of(&doc.read(ViewFilter::All, &clock)),
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
        ids_of(&doc.read(ViewFilter::All, &clock)),
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
        ids_of(&doc.read(ViewFilter::All, &clock)),
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
        ids_of(&doc.read(ViewFilter::All, &clock)),
        vec![a.clone(), b.clone(), c.clone(), d.clone()]
    );

    // Move 'b' after 'a' again: it's already there, so this is a no-op.
    let before = ids_of(&doc.read(ViewFilter::All, &clock));
    doc.apply(
        Command::Move {
            id: b.clone(),
            after: Some(a.clone()),
        },
        &clock,
        &ids,
    )
    .unwrap();
    assert_eq!(ids_of(&doc.read(ViewFilter::All, &clock)), before);
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
    let snap = doc.read(ViewFilter::All, &clock);
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
    assert_eq!(doc.read(ViewFilter::All, &clock).active_count, 2);

    doc.apply(
        Command::SetDone {
            id: a.clone(),
            done: true,
        },
        &clock,
        &ids,
    )
    .unwrap();

    let active = doc.read(ViewFilter::Active, &clock);
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

    let completed = doc.read(ViewFilter::Completed, &clock);
    assert_eq!(titles(&completed), vec!["a"]);
    assert!(completed.rows[0].done);
}

#[test]
fn undo_restores_prior_snapshot_after_each_mutating_command() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();

    let empty = ids_of(&doc.read(ViewFilter::All, &clock));
    let id = add(&mut doc, &clock, &ids, "a");
    doc.apply(Command::Undo, &clock, &ids).unwrap();
    assert_eq!(ids_of(&doc.read(ViewFilter::All, &clock)), empty);

    doc.apply(Command::Redo, &clock, &ids).unwrap();
    let after_add = doc.read(ViewFilter::All, &clock).rows;

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
    assert_eq!(doc.read(ViewFilter::All, &clock).rows, after_add);
}

#[test]
fn redo_after_undo_restores_post_command_snapshot() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    add(&mut doc, &clock, &ids, "a");
    let after_add = doc.read(ViewFilter::All, &clock).rows;

    doc.apply(Command::Undo, &clock, &ids).unwrap();
    assert!(doc.read(ViewFilter::All, &clock).rows.is_empty());

    doc.apply(Command::Redo, &clock, &ids).unwrap();
    assert_eq!(doc.read(ViewFilter::All, &clock).rows, after_add);
}

#[test]
fn undo_on_empty_history_is_noop() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let before = doc.read(ViewFilter::All, &clock);
    assert!(!before.can_undo);

    doc.apply(Command::Undo, &clock, &ids).unwrap();

    let after = doc.read(ViewFilter::All, &clock);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.rows, before.rows);
}

#[test]
fn revision_strictly_increases_across_every_mutation() {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(0);
    let ids = SeqIdSource::new();
    let mut last = doc.read(ViewFilter::All, &clock).revision;

    let mut bump = |doc: &mut Doc, cmd: Command| {
        doc.apply(cmd, &clock, &ids).unwrap();
        let rev = doc.read(ViewFilter::All, &clock).revision;
        assert!(rev > last, "revision did not increase: {last} -> {rev}");
        last = rev;
    };

    bump(
        &mut doc,
        Command::Add {
            title: "a".to_string(),
            after: None,
        },
    );
    let id = doc.read(ViewFilter::All, &clock).rows[0].id.clone();
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
    let snap = doc.read(ViewFilter::All, &clock);
    assert!(snap.rows.is_empty());
    assert_eq!(snap.active_count, 0);
}
