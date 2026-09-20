//! Integration tests for the sync-facing primitives on `App`
//! (`export_for_push`/`mark_pushed`/`import_from_pull`/`last_pulled_seq`/
//! `mark_pulled`). These don't touch any network transport — that lives in
//! `todo-sync` — they only prove the App-level contract those primitives
//! promise: incremental exports, safe retry-ability, and pulled changes
//! behaving like local edits.

use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use todo_core::{App, Command, Store};

fn db_path(dir: &TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_string()
}

fn add(app: &App, title: &str) {
    app.dispatch(Command::Add {
        title: title.to_string(),
        after: None,
    })
    .unwrap();
}

#[test]
fn peer_id_matches_the_underlying_store() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let expected = Store::open(&path).unwrap().peer_id().unwrap();

    let app = App::open(&path).unwrap();
    assert_eq!(app.peer_id(), expected);
}

#[test]
fn fresh_app_has_no_pulled_seq() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert_eq!(app.last_pulled_seq().unwrap(), None);
}

#[test]
fn mark_pulled_persists_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let app = App::open(&path).unwrap();
        app.mark_pulled(42).unwrap();
    }
    let app2 = App::open(&path).unwrap();
    assert_eq!(app2.last_pulled_seq().unwrap(), Some(42));
}

#[test]
fn fresh_app_has_no_unpushed_changes() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert!(!app.has_unpushed_changes().unwrap());
}

#[test]
fn has_unpushed_changes_is_true_after_an_edit_and_false_after_mark_pushed() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");
    assert!(app.has_unpushed_changes().unwrap());

    app.mark_pushed().unwrap();
    assert!(!app.has_unpushed_changes().unwrap());
}

#[test]
fn export_for_push_is_empty_delta_immediately_after_mark_pushed() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");

    let first = app.export_for_push().unwrap();
    assert!(!first.is_empty());
    app.mark_pushed().unwrap();

    // Nothing changed locally since mark_pushed — re-exporting must not
    // resend "a" again.
    let second = app.export_for_push().unwrap();
    let other = App::open(&db_path(&dir, "other.sqlite3")).unwrap();
    other.import_from_pull(&second).unwrap();
    assert_eq!(other.current().rows.len(), 0);
}

#[test]
fn failed_push_can_be_retried_by_re_exporting_without_mark_pushed() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");

    let attempt1 = app.export_for_push().unwrap();
    // Simulate the push failing: mark_pushed is deliberately not called.
    let attempt2 = app.export_for_push().unwrap();

    let receiver = App::open(&db_path(&dir, "receiver.sqlite3")).unwrap();
    receiver.import_from_pull(&attempt1).unwrap();
    assert_eq!(receiver.current().rows.len(), 1);

    let receiver2 = App::open(&db_path(&dir, "receiver2.sqlite3")).unwrap();
    receiver2.import_from_pull(&attempt2).unwrap();
    assert_eq!(receiver2.current().rows.len(), 1);
}

#[test]
fn import_from_pull_updates_current_snapshot_and_notifies_subscribers() {
    let dir = TempDir::new().unwrap();
    let sender = App::open(&db_path(&dir, "sender.sqlite3")).unwrap();
    add(&sender, "from sender");
    let bytes = sender.export_for_push().unwrap();

    let receiver = App::open(&db_path(&dir, "receiver.sqlite3")).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    receiver.subscribe(move |snap| seen_clone.lock().unwrap().push(snap.rows.len()));
    assert_eq!(*seen.lock().unwrap(), vec![0]); // fires immediately on subscribe

    receiver.import_from_pull(&bytes).unwrap();

    assert_eq!(receiver.current().rows.len(), 1);
    assert_eq!(receiver.current().rows[0].title, "from sender");
    assert_eq!(*seen.lock().unwrap(), vec![0, 1]);
}

#[test]
fn two_devices_converge_via_manual_push_pull_cycle() {
    // Simulates what `todo-sync`'s SyncEngine will automate: each device
    // exports what's new since its own last push, "the server" is just the
    // concatenation of both, each device imports what it doesn't have yet.
    let dir = TempDir::new().unwrap();
    let device_a = App::open(&db_path(&dir, "a.sqlite3")).unwrap();
    let device_b = App::open(&db_path(&dir, "b.sqlite3")).unwrap();

    add(&device_a, "from a");
    add(&device_b, "from b");

    let from_a = device_a.export_for_push().unwrap();
    device_a.mark_pushed().unwrap();
    let from_b = device_b.export_for_push().unwrap();
    device_b.mark_pushed().unwrap();

    device_a.import_from_pull(&from_b).unwrap();
    device_b.import_from_pull(&from_a).unwrap();

    let titles_a: Vec<_> = device_a
        .current()
        .rows
        .iter()
        .map(|r| r.title.clone())
        .collect();
    let titles_b: Vec<_> = device_b
        .current()
        .rows
        .iter()
        .map(|r| r.title.clone())
        .collect();

    assert_eq!(device_a.current().rows.len(), 2);
    assert_eq!(device_b.current().rows.len(), 2);
    assert!(titles_a.contains(&"from a".to_string()));
    assert!(titles_a.contains(&"from b".to_string()));
    assert_eq!(titles_a.len(), titles_b.len());
}
