//! Integration tests for the sync-facing primitives on `App`
//! (`export_for_push`/`mark_pushed`/`import_from_pull`/`last_pulled_seq`/
//! `mark_pulled`). These don't touch any network transport — that lives in
//! `remember-sync` — they only prove the App-level contract those primitives
//! promise: incremental exports, safe retry-ability, and pulled changes
//! behaving like local edits.

use std::sync::{Arc, Mutex};

use remember_core::{App, Command, CoreError, Store};
use tempfile::TempDir;

fn db_path(dir: &TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_string()
}

fn add(app: &App, title: &str) {
    app.dispatch(Command::Add {
        title: title.to_string(),
        after: None,
        due: None,
        list_id: None,
        parent_id: None,
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

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn fresh_app_has_never_synced() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert_eq!(app.last_synced_at().unwrap(), None);
}

#[test]
fn mark_synced_records_now_and_persists_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let before = unix_now();
    {
        let app = App::open(&path).unwrap();
        app.mark_synced().unwrap();
    }
    let after = unix_now();

    let synced_at = App::open(&path).unwrap().last_synced_at().unwrap().unwrap();
    assert!((before..=after).contains(&synced_at));
}

/// Even before the caller ever dispatches a `Command`, a fresh app already
/// has something worth pushing — bootstrapping the default list is itself
/// real document content, not a local-only concern — which matters for
/// cross-device convergence: a second device must learn about the first
/// device's default list (same fixed id, so this converges harmlessly) via
/// the very first sync round, not just whatever the user types afterward.
#[test]
fn fresh_app_has_unpushed_bootstrap_changes() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert!(app.has_unpushed_changes().unwrap());
}

#[test]
fn has_unpushed_changes_is_true_after_an_edit_and_false_after_mark_pushed() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");
    assert!(app.has_unpushed_changes().unwrap());

    let batch = app.export_for_push().unwrap();
    app.mark_pushed(&batch).unwrap();
    assert!(!app.has_unpushed_changes().unwrap());
}

#[test]
fn export_for_push_is_empty_delta_immediately_after_mark_pushed() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");

    let first = app.export_for_push().unwrap();
    assert!(!first.bytes.is_empty());
    app.mark_pushed(&first).unwrap();

    // Nothing changed locally since mark_pushed — re-exporting must not
    // resend "a" again.
    let second = app.export_for_push().unwrap();
    let other = App::open(&db_path(&dir, "other.sqlite3")).unwrap();
    other.import_from_pull(&second.bytes).unwrap();
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
    receiver.import_from_pull(&attempt1.bytes).unwrap();
    assert_eq!(receiver.current().rows.len(), 1);

    let receiver2 = App::open(&db_path(&dir, "receiver2.sqlite3")).unwrap();
    receiver2.import_from_pull(&attempt2.bytes).unwrap();
    assert_eq!(receiver2.current().rows.len(), 1);
}

#[test]
fn import_from_pull_updates_current_snapshot_and_notifies_subscribers() {
    let dir = TempDir::new().unwrap();
    let sender = App::open(&db_path(&dir, "sender.sqlite3")).unwrap();
    add(&sender, "from sender");
    let bytes = sender.export_for_push().unwrap().bytes;

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
    // Simulates what `remember-sync`'s SyncEngine will automate: each device
    // exports what's new since its own last push, "the server" is just the
    // concatenation of both, each device imports what it doesn't have yet.
    let dir = TempDir::new().unwrap();
    let device_a = App::open(&db_path(&dir, "a.sqlite3")).unwrap();
    let device_b = App::open(&db_path(&dir, "b.sqlite3")).unwrap();

    add(&device_a, "from a");
    add(&device_b, "from b");

    let from_a = device_a.export_for_push().unwrap();
    device_a.mark_pushed(&from_a).unwrap();
    let from_b = device_b.export_for_push().unwrap();
    device_b.mark_pushed(&from_b).unwrap();

    device_a.import_from_pull(&from_b.bytes).unwrap();
    device_b.import_from_pull(&from_a.bytes).unwrap();

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

#[test]
fn first_sync_account_binding_keeps_local_data() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "made before signing in");
    let peer = app.peer_id();

    assert!(!app.bind_sync_account("user-a").unwrap());

    assert_eq!(app.sync_account().unwrap().as_deref(), Some("user-a"));
    assert_eq!(app.current().rows[0].title, "made before signing in");
    assert_eq!(app.peer_id(), peer);
}

#[test]
fn rebinding_the_same_sync_account_is_a_no_op() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    app.bind_sync_account("user-a").unwrap();
    add(&app, "a's task");
    app.mark_pulled(7).unwrap();

    assert!(!app.bind_sync_account("user-a").unwrap());

    assert_eq!(app.current().rows.len(), 1);
    assert_eq!(app.last_pulled_seq().unwrap(), Some(7));
}

#[test]
fn binding_a_different_sync_account_resets_local_state() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    app.set_local_offset_seconds(3_600);
    app.bind_sync_account("user-a").unwrap();
    add(&app, "a's task");
    app.dispatch(Command::AddList {
        name: "A's list".to_string(),
        after: None,
    })
    .unwrap();
    let a_list = app
        .current()
        .lists
        .into_iter()
        .find(|l| l.name == "A's list")
        .unwrap()
        .id;
    app.set_current_list(a_list.clone()).unwrap();
    app.mark_pulled(7).unwrap();
    app.mark_pushed(&app.export_for_push().unwrap()).unwrap();
    app.mark_synced().unwrap();
    let a_peer = app.peer_id();
    let due_before = app.detect_due("x today").unwrap().due;

    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&snapshots);
    app.subscribe(move |s| seen.lock().unwrap().push(s.clone()));

    assert!(app.bind_sync_account("user-b").unwrap());

    let snap = app.current();
    assert!(snap.rows.is_empty());
    assert!(snap.lists.iter().all(|l| l.id != a_list));
    assert_ne!(snap.current_list, a_list);
    assert_ne!(app.peer_id(), a_peer);
    assert_eq!(app.last_pulled_seq().unwrap(), None);
    // "Last synced" was about A; B has never synced here.
    assert_eq!(app.last_synced_at().unwrap(), None);
    assert_eq!(app.sync_account().unwrap().as_deref(), Some("user-b"));
    // Nothing of A's counts as already pushed -- only B's fresh bootstrap
    // list is waiting.
    assert!(app.has_unpushed_changes().unwrap());
    // The device's local offset is not account data; it survives.
    assert_eq!(app.detect_due("x today").unwrap().due, due_before);
    // Subscribers see the reset like any other change.
    assert!(snapshots.lock().unwrap().last().unwrap().rows.is_empty());
}

#[test]
fn a_sync_account_reset_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let b_peer = {
        let app = App::open(&path).unwrap();
        app.bind_sync_account("user-a").unwrap();
        add(&app, "a's task");
        app.flush().unwrap();
        app.bind_sync_account("user-b").unwrap();
        app.peer_id()
    };

    let app = App::open(&path).unwrap();
    assert!(app.current().rows.is_empty());
    assert_eq!(app.peer_id(), b_peer);
    assert_eq!(app.sync_account().unwrap().as_deref(), Some("user-b"));
}

#[test]
fn a_failed_sync_account_reset_leaves_the_previous_account_intact() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let app = App::open(&path).unwrap();
    app.bind_sync_account("user-a").unwrap();
    add(&app, "a's task");
    app.mark_pulled(7).unwrap();
    let a_peer = app.peer_id();

    // Makes the reset's transaction fail partway (its first statement
    // deletes the sync cursors), the way a full disk or I/O error would.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_reset BEFORE DELETE ON meta
             BEGIN SELECT RAISE(ABORT, 'simulated failure'); END;",
        )
        .unwrap();

    assert!(matches!(
        app.bind_sync_account("user-b"),
        Err(CoreError::Storage(_))
    ));

    // Nothing half-applied, in memory or on disk.
    assert_eq!(app.current().rows[0].title, "a's task");
    assert_eq!(app.peer_id(), a_peer);
    assert_eq!(app.sync_account().unwrap().as_deref(), Some("user-a"));
    assert_eq!(app.last_pulled_seq().unwrap(), Some(7));
}

fn titles(app: &App) -> Vec<String> {
    let mut titles: Vec<String> = app.current().rows.into_iter().map(|r| r.title).collect();
    titles.sort();
    titles
}

#[test]
fn a_compaction_snapshot_rebuilds_the_document_on_a_fresh_device() {
    let dir = TempDir::new().unwrap();
    let source = App::open(&db_path(&dir, "source.sqlite3")).unwrap();
    add(&source, "one");
    add(&source, "two");

    let fresh = App::open(&db_path(&dir, "fresh.sqlite3")).unwrap();
    fresh
        .import_from_pull(&source.export_for_compaction().unwrap())
        .unwrap();

    assert_eq!(titles(&fresh), vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn a_compaction_snapshot_merges_into_a_device_with_history_and_edits_of_its_own() {
    let dir = TempDir::new().unwrap();
    let source = App::open(&db_path(&dir, "source.sqlite3")).unwrap();
    let lagging = App::open(&db_path(&dir, "lagging.sqlite3")).unwrap();
    add(&source, "shared");
    lagging
        .import_from_pull(&source.export_for_push().unwrap().bytes)
        .unwrap();
    add(&source, "only in snapshot");
    add(&lagging, "only on lagging device");

    lagging
        .import_from_pull(&source.export_for_compaction().unwrap())
        .unwrap();

    assert_eq!(
        titles(&lagging),
        vec![
            "only in snapshot".to_string(),
            "only on lagging device".to_string(),
            "shared".to_string(),
        ]
    );
    // The lagging device's own edit survives the import and is still
    // pending, so the next round pushes it.
    assert!(lagging.has_unpushed_changes().unwrap());
}

#[test]
fn an_edit_made_while_a_push_is_in_flight_goes_in_the_next_push() {
    let dir = TempDir::new().unwrap();
    let sender = App::open(&db_path(&dir, "sender.sqlite3")).unwrap();
    let receiver = App::open(&db_path(&dir, "receiver.sqlite3")).unwrap();

    add(&sender, "one");
    let first = sender.export_for_push().unwrap();
    add(&sender, "two"); // while `first` is still on its way to the server
    sender.mark_pushed(&first).unwrap();
    receiver.import_from_pull(&first.bytes).unwrap();

    assert!(sender.has_unpushed_changes().unwrap());
    add(&sender, "three");
    receiver
        .import_from_pull(&sender.export_for_push().unwrap().bytes)
        .unwrap();

    assert_eq!(titles(&receiver), vec!["one", "three", "two"]);
    assert!(!receiver.has_missing_changes());
}

#[test]
fn changes_that_build_on_missing_ones_are_reported_until_the_gap_is_filled() {
    let dir = TempDir::new().unwrap();
    let sender = App::open(&db_path(&dir, "sender.sqlite3")).unwrap();
    let receiver = App::open(&db_path(&dir, "receiver.sqlite3")).unwrap();
    add(&sender, "one");
    let first = sender.export_for_push().unwrap();
    sender.mark_pushed(&first).unwrap();
    receiver.import_from_pull(&first.bytes).unwrap();
    assert!(!receiver.has_missing_changes());

    add(&sender, "lost");
    let lost = sender.export_for_push().unwrap();
    sender.mark_pushed(&lost).unwrap();
    add(&sender, "after the gap");
    receiver
        .import_from_pull(&sender.export_for_push().unwrap().bytes)
        .unwrap();

    assert!(receiver.has_missing_changes());
    assert_eq!(titles(&receiver), vec!["one"]);

    receiver.import_from_pull(&lost.bytes).unwrap();

    assert!(!receiver.has_missing_changes());
    assert_eq!(titles(&receiver), vec!["after the gap", "lost", "one"]);
}

/// Opens `path` as an install from before `repush_own_history_once`
/// existed would look: it pushed `titles` and recorded them as pushed, but
/// never ran the repair.
fn open_install_from_before_the_repair(path: &str, titles: &[&str]) -> App {
    {
        let app = App::open(path).unwrap();
        for title in titles {
            add(&app, title);
        }
        app.mark_pushed(&app.export_for_push().unwrap()).unwrap();
    }
    rusqlite::Connection::open(path)
        .unwrap()
        .execute("DELETE FROM meta WHERE key = 'own_history_repushed'", [])
        .unwrap();
    App::open(path).unwrap()
}

#[test]
fn an_install_that_pushed_before_resends_only_its_own_history_once() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "old.sqlite3");
    let other = App::open(&db_path(&dir, "other.sqlite3")).unwrap();
    add(&other, "from another device");
    let from_other = other.export_for_push().unwrap().bytes;
    {
        let app = App::open(&path).unwrap();
        app.import_from_pull(&from_other).unwrap();
    }

    let app = open_install_from_before_the_repair(&path, &["mine", "also mine"]);

    assert!(app.has_unpushed_changes().unwrap());
    let repush = app.export_for_push().unwrap();
    // Its changes build on the other device's, which every receiver
    // already has from the log -- but they aren't resent.
    let peer = App::open(&db_path(&dir, "peer.sqlite3")).unwrap();
    peer.import_from_pull(&from_other).unwrap();
    peer.import_from_pull(&repush.bytes).unwrap();
    assert_eq!(
        titles(&peer),
        vec!["also mine", "from another device", "mine"]
    );
    let without_other = App::open(&db_path(&dir, "without.sqlite3")).unwrap();
    without_other.import_from_pull(&repush.bytes).unwrap();
    assert!(without_other.has_missing_changes());
    app.mark_pushed(&repush).unwrap();
    drop(app);

    let reopened = App::open(&path).unwrap();
    assert!(!reopened.has_unpushed_changes().unwrap());
}

#[test]
fn the_resend_fills_a_gap_left_by_an_edit_lost_mid_push() {
    let dir = TempDir::new().unwrap();
    let sender_path = db_path(&dir, "sender.sqlite3");
    let receiver_path = db_path(&dir, "receiver.sqlite3");
    {
        let sender = App::open(&sender_path).unwrap();
        let receiver = App::open(&receiver_path).unwrap();
        add(&sender, "one");
        let first = sender.export_for_push().unwrap();
        sender.mark_pushed(&first).unwrap();
        receiver.import_from_pull(&first.bytes).unwrap();
        // "lost" is recorded as pushed but never reaches the server.
        add(&sender, "lost");
        let lost = sender.export_for_push().unwrap();
        sender.mark_pushed(&lost).unwrap();
        add(&sender, "after");
        receiver
            .import_from_pull(&sender.export_for_push().unwrap().bytes)
            .unwrap();
        assert!(receiver.has_missing_changes());
    }
    // Loro keeps held-back changes in memory only: "after" is gone too.
    let receiver = App::open(&receiver_path).unwrap();
    assert!(!receiver.has_missing_changes());
    assert_eq!(titles(&receiver), vec!["one"]);
    let sender = open_install_from_before_the_repair(&sender_path, &[]);

    receiver
        .import_from_pull(&sender.export_for_push().unwrap().bytes)
        .unwrap();

    assert!(!receiver.has_missing_changes());
    assert_eq!(titles(&receiver), vec!["after", "lost", "one"]);
}

#[test]
fn a_fresh_install_never_pushed_so_has_nothing_to_resend() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "fresh.sqlite3");
    drop(App::open(&path).unwrap());
    let app = App::open(&path).unwrap();
    // Only the bootstrap list, same as any fresh install's first push.
    let batch = app.export_for_push().unwrap();
    app.mark_pushed(&batch).unwrap();
    assert!(!app.has_unpushed_changes().unwrap());
}
