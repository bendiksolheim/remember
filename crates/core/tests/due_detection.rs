//! Integration tests for `App::detect_due`/`App::set_local_offset_seconds`.
//! `App` always runs on the real `SystemClock` (no injectable clock at this
//! layer, unlike `Doc`, which the FixedClock-based tests in `doc.rs`
//! exercise directly) — so these deliberately assert only what's true
//! regardless of the actual wall-clock date: "today"/"tomorrow" labels are
//! self-consistent by construction (the same offset resolves both the
//! target day and the label), not tied to a specific calendar date.

use std::sync::{Arc, Mutex};

use remember_core::App;
use tempfile::TempDir;

fn app(dir: &TempDir) -> App {
    App::open(dir.path().join("todo.sqlite3").to_str().unwrap()).unwrap()
}

#[test]
fn detects_today_and_strips_it_from_the_title() {
    let dir = TempDir::new().unwrap();
    let app = app(&dir);
    let detection = app.detect_due("Buy milk today").unwrap();
    assert_eq!(detection.stripped_title, "Buy milk");
    assert_eq!(detection.label, "Today");
}

#[test]
fn detects_tomorrow() {
    let dir = TempDir::new().unwrap();
    let app = app(&dir);
    let detection = app.detect_due("Buy milk tomorrow").unwrap();
    assert_eq!(detection.stripped_title, "Buy milk");
    assert_eq!(detection.label, "Tomorrow");
}

#[test]
fn no_recognized_phrase_is_none() {
    let dir = TempDir::new().unwrap();
    let app = app(&dir);
    assert!(app.detect_due("Buy milk").is_none());
    assert!(app.detect_due("").is_none());
}

#[test]
fn detection_stays_self_consistent_after_changing_the_local_offset() {
    // Whatever offset is in effect, "today" resolves to the same day that
    // offset's own due_label would call "Today" — that invariant holds
    // regardless of the real wall-clock date, which this test never reads.
    let dir = TempDir::new().unwrap();
    let app = app(&dir);
    app.set_local_offset_seconds(-8 * 3_600);
    let detection = app.detect_due("Call dentist today").unwrap();
    assert_eq!(detection.label, "Today");

    app.set_local_offset_seconds(12 * 3_600);
    let detection = app.detect_due("Call dentist today").unwrap();
    assert_eq!(detection.label, "Today");
}

#[test]
fn set_local_offset_seconds_notifies_subscribers_without_a_dispatch() {
    let dir = TempDir::new().unwrap();
    let app = app(&dir);

    let calls = Arc::new(Mutex::new(0));
    let calls_clone = Arc::clone(&calls);
    app.subscribe(move |_snap| *calls_clone.lock().unwrap() += 1);
    assert_eq!(*calls.lock().unwrap(), 1); // fires immediately on subscribe

    app.set_local_offset_seconds(3_600);
    assert_eq!(*calls.lock().unwrap(), 2);
}
