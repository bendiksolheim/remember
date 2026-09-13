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
