use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;
use todo_core::{App, Command, CoreError, Store, ViewFilter};

fn db_path(dir: &TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_string()
}

fn add(app: &App, title: &str) {
    app.dispatch(Command::Add {
        title: title.to_string(),
        after: None,
        due: None,
        list_id: None,
    })
    .unwrap();
}

#[test]
fn fresh_db_generates_nonzero_peer_id() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert_ne!(store.peer_id().unwrap(), 0);
}

#[test]
fn reopening_same_path_returns_same_peer_id() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let id1 = Store::open(&path).unwrap().peer_id().unwrap();
    let id2 = Store::open(&path).unwrap().peer_id().unwrap();
    assert_eq!(id1, id2);
}

#[test]
fn different_paths_get_different_peer_ids() {
    let dir = TempDir::new().unwrap();
    let id1 = Store::open(&db_path(&dir, "a.sqlite3"))
        .unwrap()
        .peer_id()
        .unwrap();
    let id2 = Store::open(&db_path(&dir, "b.sqlite3"))
        .unwrap()
        .peer_id()
        .unwrap();
    assert_ne!(id1, id2);
}

#[test]
fn data_survives_drop_and_reopen() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let app = App::open(&path).unwrap();
        add(&app, "a");
        add(&app, "b");
        // Dropped here — Drop flushes explicitly.
    }
    let app2 = App::open(&path).unwrap();
    let snap = app2.current();
    assert_eq!(snap.rows.len(), 2);
    assert_eq!(snap.active_count, 2);
}

#[test]
fn explicit_flush_then_reopen_without_dropping_sees_data() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let app1 = App::open(&path).unwrap();
    add(&app1, "a");
    app1.flush().unwrap();

    let app2 = App::open(&path).unwrap(); // app1 is still alive here
    assert_eq!(app2.current().rows.len(), 1);
}

#[test]
fn debounced_write_lands_without_explicit_flush() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let app1 = App::open(&path).unwrap();
    add(&app1, "a");
    // No explicit flush() call — only the 2s debounced background writer.
    thread::sleep(Duration::from_millis(2_300));

    let app2 = App::open(&path).unwrap();
    assert_eq!(app2.current().rows.len(), 1);
}

#[test]
fn opening_nonexistent_directory_is_storage_error() {
    let err = App::open("/this/dir/does/not/exist/todo.sqlite3")
        .err()
        .unwrap();
    assert!(matches!(err, CoreError::Storage(_)));
}

#[test]
fn opening_garbage_file_is_storage_error() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "garbage.sqlite3");
    std::fs::write(&path, b"not a sqlite database").unwrap();
    let err = App::open(&path).err().unwrap();
    assert!(matches!(err, CoreError::Storage(_)));
}

#[test]
fn corrupt_snapshot_blob_is_document_error() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let app = App::open(&path).unwrap();
        add(&app, "a");
        app.flush().unwrap();
    }

    // Corrupt the stored blob directly, bypassing Doc/App entirely.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE doc SET snapshot = ?1 WHERE id = 1",
        (b"not a loro snapshot".to_vec(),),
    )
    .unwrap();
    drop(conn);

    let err = App::open(&path).err().unwrap();
    assert!(matches!(err, CoreError::Document(_)));
}

#[test]
fn journal_mode_is_wal() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    let _store = Store::open(&path).unwrap();

    let conn = rusqlite::Connection::open(&path).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn set_view_changes_what_current_returns() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");
    let id = app.current().rows[0].id.clone();
    app.dispatch(Command::SetDone { id, done: true }).unwrap();

    assert_eq!(app.current().rows.len(), 1); // default view is All

    app.set_view(ViewFilter::Active);
    assert!(app.current().rows.is_empty());

    app.set_view(ViewFilter::Completed);
    assert_eq!(app.current().rows.len(), 1);
}

#[test]
fn set_view_notifies_existing_subscribers() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    add(&app, "a");
    let id = app.current().rows[0].id.clone();
    app.dispatch(Command::SetDone { id, done: true }).unwrap();

    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = Arc::clone(&received);
    app.subscribe(move |snap| received_clone.lock().unwrap().push(snap.rows.len()));
    // subscribe() itself fires once immediately with the current (All) view.
    assert_eq!(*received.lock().unwrap(), vec![1]);

    app.set_view(ViewFilter::Active);
    // set_view must push a fresh, filtered snapshot without a dispatch().
    assert_eq!(*received.lock().unwrap(), vec![1, 0]);
}

#[test]
fn corrupt_peer_id_blob_is_storage_error() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let _store = Store::open(&path).unwrap();
    }

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('peer_id', ?1)",
        (b"short".to_vec(),), // not 8 bytes
    )
    .unwrap();
    drop(conn);

    let err = Store::open(&path).unwrap().peer_id().err().unwrap();
    assert!(matches!(err, CoreError::Storage(_)));
}

#[test]
fn fresh_store_has_no_sync_cursors() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert_eq!(store.load_pushed_vv().unwrap(), None);
    assert_eq!(store.load_pulled_seq().unwrap(), None);
}

#[test]
fn pushed_vv_round_trips_and_can_be_overwritten() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&db_path(&dir, "todo.sqlite3")).unwrap();

    store.save_pushed_vv(b"first").unwrap();
    assert_eq!(store.load_pushed_vv().unwrap(), Some(b"first".to_vec()));

    store.save_pushed_vv(b"second").unwrap();
    assert_eq!(store.load_pushed_vv().unwrap(), Some(b"second".to_vec()));
}

#[test]
fn pulled_seq_round_trips_and_can_be_overwritten() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&db_path(&dir, "todo.sqlite3")).unwrap();

    store.save_pulled_seq(7).unwrap();
    assert_eq!(store.load_pulled_seq().unwrap(), Some(7));

    store.save_pulled_seq(8).unwrap();
    assert_eq!(store.load_pulled_seq().unwrap(), Some(8));
}

#[test]
fn corrupt_pulled_seq_blob_is_storage_error() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let _store = Store::open(&path).unwrap();
    }

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('pulled_seq', ?1)",
        (b"short".to_vec(),), // not 8 bytes
    )
    .unwrap();
    drop(conn);

    let err = Store::open(&path).unwrap().load_pulled_seq().err().unwrap();
    assert!(matches!(err, CoreError::Storage(_)));
}

#[test]
fn subscribe_fires_immediately_then_on_every_dispatch() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();

    let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    app.subscribe(move |snap| seen_clone.lock().unwrap().push(snap.rows.len()));

    // Fired immediately on subscribe, with the (empty) current snapshot.
    assert_eq!(*seen.lock().unwrap(), vec![0]);

    add(&app, "a");
    add(&app, "b");
    assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2]);
}

#[test]
fn fresh_store_has_no_current_list() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert_eq!(store.load_current_list().unwrap(), None);
}

#[test]
fn current_list_round_trips_and_can_be_overwritten() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&db_path(&dir, "todo.sqlite3")).unwrap();

    store.save_current_list("work").unwrap();
    assert_eq!(store.load_current_list().unwrap(), Some("work".to_string()));

    store.save_current_list("personal").unwrap();
    assert_eq!(
        store.load_current_list().unwrap(),
        Some("personal".to_string())
    );
}

#[test]
fn corrupt_current_list_blob_is_storage_error() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let _store = Store::open(&path).unwrap();
    }

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('current_list', ?1)",
        (vec![0xFF, 0xFE],), // not valid UTF-8
    )
    .unwrap();
    drop(conn);

    let err = Store::open(&path)
        .unwrap()
        .load_current_list()
        .err()
        .unwrap();
    assert!(matches!(err, CoreError::Storage(_)));
}

#[test]
fn set_current_list_persists_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir, "todo.sqlite3");
    {
        let app = App::open(&path).unwrap();
        app.set_current_list("work".to_string()).unwrap();
    }
    let app2 = App::open(&path).unwrap();
    assert_eq!(app2.current().current_list, "work");
}

#[test]
fn set_current_list_notifies_existing_subscribers() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    app.subscribe(move |snap| seen_clone.lock().unwrap().push(snap.current_list.clone()));

    app.set_current_list("work".to_string()).unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["default".to_string(), "work".to_string()]
    );
}

#[test]
fn current_list_defaults_to_the_default_list_on_a_fresh_install() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    assert_eq!(app.current().current_list, "default");
    add(&app, "a");
    assert_eq!(app.current().rows[0].list_id, "default");
}

/// Deleting the list `current_list` points to — whether it's the one new
/// captures land in, the one being viewed, or (now that there's only one
/// such pointer) both at once — must not leave the app with nowhere to
/// read from or capture into. It should fall back to another list still
/// in the roster rather than erroring with `NotFound` on the next `Add`.
#[test]
fn deleting_the_current_list_falls_back_to_a_remaining_list() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    let default_id = app.current().current_list;

    app.dispatch(Command::AddList {
        name: "Work".to_string(),
        after: None,
    })
    .unwrap();
    let work = app.current().lists[0].id.clone();
    app.set_current_list(work.clone()).unwrap();
    assert_eq!(app.current().current_list, work);

    app.dispatch(Command::DeleteList { id: work }).unwrap();

    let fallback = app.current().current_list;
    assert_eq!(fallback, default_id);
    add(&app, "a");
    assert_eq!(app.current().rows[0].list_id, fallback);
}

/// Deleting a list that `current_list` doesn't point to must leave it
/// exactly as it was.
#[test]
fn deleting_an_unrelated_list_leaves_current_list_untouched() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    let default_id = app.current().current_list;

    app.dispatch(Command::AddList {
        name: "Work".to_string(),
        after: None,
    })
    .unwrap();
    let work = app.current().lists[0].id.clone();

    app.dispatch(Command::DeleteList { id: work }).unwrap();

    assert_eq!(app.current().current_list, default_id);
}

/// `App::dispatch` must surface a failing command's error rather than
/// swallowing it — proven here with an unknown list id, which
/// `Doc::apply_delete_list` rejects with `NotFound`.
#[test]
fn dispatch_surfaces_the_underlying_apply_error() {
    let dir = TempDir::new().unwrap();
    let app = App::open(&db_path(&dir, "todo.sqlite3")).unwrap();
    let err = app
        .dispatch(Command::DeleteList {
            id: "no-such-list".to_string(),
        })
        .unwrap_err();
    assert_eq!(err, CoreError::NotFound("no-such-list".to_string()));
}
