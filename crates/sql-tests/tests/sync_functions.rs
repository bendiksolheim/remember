//! `supabase/schema.sql`'s `sync_push`/`sync_pull`/`sync_compact` against a
//! real Postgres, with `supabase/test/supabase_shim.sql` standing in for the
//! Supabase parts (`auth.uid()`, the client roles). Each test calls the
//! functions the way PostgREST does: one transaction per request, as role
//! `authenticated`, with the JWT's claims in `request.jwt.claims`. Every
//! test makes its own users, so they can share one database and run in
//! parallel.
//!
//! Run with `cargo xtask pgtest`, which starts the database and points
//! `REMEMBER_SYNC_PG_URL` at it.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use postgres::error::SqlState;
use postgres::{Client, NoTls, Transaction};
use serde_json::{json, Value};

const NEEDS_POSTGRES: &str = "needs Postgres: run with `cargo xtask pgtest`";

fn connect() -> Client {
    let url = std::env::var("REMEMBER_SYNC_PG_URL")
        .unwrap_or_else(|_| panic!("REMEMBER_SYNC_PG_URL is not set; {NEEDS_POSTGRES}"));
    Client::connect(&url, NoTls).unwrap()
}

fn new_user(db: &mut Client) -> String {
    db.query_one(
        "insert into auth.users default values returning id::text",
        &[],
    )
    .unwrap()
    .get(0)
}

/// Opens a transaction that acts as `role` signed in as `user` (`None`: no
/// JWT at all), like PostgREST does for each request.
fn begin_as<'a>(db: &'a mut Client, role: &str, user: Option<&str>) -> Transaction<'a> {
    let mut tx = db.transaction().unwrap();
    tx.batch_execute(&format!("set local role {role}")).unwrap();
    if let Some(user) = user {
        tx.execute(
            "select set_config('request.jwt.claims', $1, true)",
            &[&json!({ "sub": user, "role": "authenticated" }).to_string()],
        )
        .unwrap();
    }
    tx
}

/// Runs `sql` as one `authenticated` request from `user`, committing it if
/// it succeeds.
fn request(
    db: &mut Client,
    user: Option<&str>,
    sql: &str,
    params: &[&(dyn postgres::types::ToSql + Sync)],
) -> Result<postgres::Row, postgres::Error> {
    let mut tx = begin_as(db, "authenticated", user);
    let row = tx.query_one(sql, params)?;
    tx.commit()?;
    Ok(row)
}

fn push(db: &mut Client, user: &str, payload: &[u8]) -> i64 {
    request(
        db,
        Some(user),
        "select sync_push($1, $2)",
        &[&7i64, &payload],
    )
    .unwrap()
    .get(0)
}

fn pull(db: &mut Client, user: &str, since: Option<i64>) -> Value {
    request(db, Some(user), "select sync_pull($1)", &[&since])
        .unwrap()
        .get(0)
}

fn compact(db: &mut Client, user: &str, as_of_seq: i64, payload: &[u8]) -> bool {
    request(
        db,
        Some(user),
        "select sync_compact($1, $2)",
        &[&payload, &as_of_seq],
    )
    .unwrap()
    .get(0)
}

fn seqs(page: &Value) -> Vec<i64> {
    page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["seq"].as_i64().unwrap())
        .collect()
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn pushed_rows_come_back_in_order_with_hex_payloads() {
    let mut db = connect();
    let user = new_user(&mut db);
    let first = push(&mut db, &user, &[1, 2]);
    let second = push(&mut db, &user, &[0xff]);
    assert!(second > first);

    let page = pull(&mut db, &user, None);

    assert_eq!(
        page,
        json!({
            "snapshot": null,
            "rows": [
                { "seq": first, "payload": "\\x0102" },
                { "seq": second, "payload": "\\xff" },
            ],
            "has_more": false,
            "log_len": 2,
        })
    );
    assert_eq!(seqs(&pull(&mut db, &user, Some(first))), vec![second]);
    assert_eq!(seqs(&pull(&mut db, &user, Some(second))), Vec::<i64>::new());
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn compaction_replaces_the_rows_it_covers_with_the_snapshot() {
    let mut db = connect();
    let user = new_user(&mut db);
    let s1 = push(&mut db, &user, &[1]);
    let s2 = push(&mut db, &user, &[2]);
    let s3 = push(&mut db, &user, &[3]);

    assert!(compact(&mut db, &user, s2, &[0xaa]));

    let snapshot = json!({ "payload": "\\xaa", "as_of_seq": s2 });
    // Never pulled, or behind the snapshot: the rows it needs are gone, so
    // it gets the snapshot, then the rows after it.
    for since in [None, Some(s1)] {
        let page = pull(&mut db, &user, since);
        assert_eq!(page["snapshot"], snapshot, "since {since:?}");
        assert_eq!(seqs(&page), vec![s3], "since {since:?}");
        assert_eq!(page["log_len"], 1);
    }
    // Already past it: no snapshot.
    let page = pull(&mut db, &user, Some(s2));
    assert_eq!(page["snapshot"], Value::Null);
    assert_eq!(seqs(&page), vec![s3]);
    assert_eq!(pull(&mut db, &user, Some(s3))["snapshot"], Value::Null);
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn a_compaction_at_a_seq_the_log_no_longer_holds_changes_nothing() {
    let mut db = connect();
    let user = new_user(&mut db);
    let s1 = push(&mut db, &user, &[1]);
    let s2 = push(&mut db, &user, &[2]);
    let s3 = push(&mut db, &user, &[3]);
    assert!(compact(&mut db, &user, s2, &[0xaa]));

    // Stale: another device already compacted past s1.
    assert!(!compact(&mut db, &user, s1, &[0xbb]));
    // Already the snapshot's seq.
    assert!(!compact(&mut db, &user, s2, &[0xbb]));
    // Not a seq this account has ever had.
    assert!(!compact(&mut db, &user, s3 + 1_000_000, &[0xbb]));

    let page = pull(&mut db, &user, None);
    assert_eq!(page["snapshot"]["payload"], "\\xaa");
    assert_eq!(seqs(&page), vec![s3]);
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn accounts_never_see_or_compact_each_others_rows() {
    let mut db = connect();
    let alice = new_user(&mut db);
    let bob = new_user(&mut db);
    let alices = push(&mut db, &alice, &[1]);

    assert_eq!(seqs(&pull(&mut db, &bob, None)), Vec::<i64>::new());
    assert!(!compact(&mut db, &bob, alices, &[0xbb]));

    assert_eq!(seqs(&pull(&mut db, &alice, None)), vec![alices]);
    assert_eq!(pull(&mut db, &alice, None)["snapshot"], Value::Null);
    assert_eq!(pull(&mut db, &bob, None)["snapshot"], Value::Null);
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn pull_pages_at_a_thousand_rows() {
    let mut db = connect();
    let user = new_user(&mut db);
    {
        let mut tx = begin_as(&mut db, "authenticated", Some(&user));
        tx.batch_execute("select sync_push(7, '\\x00') from generate_series(1, 1001)")
            .unwrap();
        tx.commit().unwrap();
    }

    let first = pull(&mut db, &user, None);
    assert_eq!(seqs(&first).len(), 1000);
    assert_eq!(first["has_more"], true);
    assert_eq!(first["log_len"], 1001);

    let cursor = *seqs(&first).last().unwrap();
    let second = pull(&mut db, &user, Some(cursor));
    assert_eq!(seqs(&second).len(), 1);
    assert_eq!(second["has_more"], false);
}

/// The reason `sync_push` takes a lock: without it, seqs could commit out
/// of order, and a pull in between would skip the lower one for good.
#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn pushes_to_one_account_wait_for_each_other_but_not_for_other_accounts() {
    let mut db = connect();
    let alice = new_user(&mut db);
    let bob = new_user(&mut db);

    let mut first_db = connect();
    let mut first = begin_as(&mut first_db, "authenticated", Some(&alice));
    let first_seq: i64 = first
        .query_one("select sync_push(1, '\\x01')", &[])
        .unwrap()
        .get(0);

    // Another of Alice's devices pushes while the first push is uncommitted.
    let (done_tx, done_rx) = mpsc::channel();
    let second_alice = alice.clone();
    let second = thread::spawn(move || {
        let mut db = connect();
        let seq = push(&mut db, &second_alice, &[2]);
        done_tx.send(seq).unwrap();
    });
    assert!(
        done_rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "the second push should wait for the first to commit"
    );

    // Bob isn't held up by Alice's lock.
    push(&mut db, &bob, &[3]);

    first.commit().unwrap();
    let second_seq = done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    second.join().unwrap();
    assert!(second_seq > first_seq);
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn clients_cannot_touch_the_tables_directly() {
    let mut db = connect();
    let user = new_user(&mut db);
    for sql in [
        "select count(*) from sync_log",
        "select count(*) from sync_snapshots",
        "insert into sync_log (device_id, payload) values (1, '\\x00') returning seq",
        "delete from sync_log returning seq",
    ] {
        let err = request(&mut db, Some(&user), sql, &[]).unwrap_err();
        assert_eq!(
            err.code(),
            Some(&SqlState::INSUFFICIENT_PRIVILEGE),
            "{sql}: {err}"
        );
    }
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn calls_without_a_signed_in_user_are_rejected() {
    let mut db = connect();
    for sql in [
        "select sync_push(1, '\\x00')",
        "select sync_pull(null)",
        "select sync_compact('\\x00', 1)",
    ] {
        let err = request(&mut db, None, sql, &[]).unwrap_err();
        assert_eq!(
            err.code(),
            Some(&SqlState::INVALID_AUTHORIZATION_SPECIFICATION),
            "{sql}: {err}"
        );
    }
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn only_signed_in_clients_may_call_the_functions() {
    let mut db = connect();
    let user = new_user(&mut db);

    let mut tx = begin_as(&mut db, "anon", Some(&user));
    let err = tx.query_one("select sync_pull(null)", &[]).unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::INSUFFICIENT_PRIVILEGE), "{err}");
    drop(tx);

    // The lock helper is internal: calling it directly would let a client
    // hold its account's lock for as long as it liked.
    let err = request(&mut db, Some(&user), "select sync_lock_account()", &[]).unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::INSUFFICIENT_PRIVILEGE), "{err}");
}

#[test]
#[ignore = "needs Postgres: run with `cargo xtask pgtest`"]
fn deleting_a_user_deletes_their_log_and_snapshot() {
    let mut db = connect();
    let user = new_user(&mut db);
    let seq = push(&mut db, &user, &[1]);
    push(&mut db, &user, &[2]);
    assert!(compact(&mut db, &user, seq, &[0xaa]));

    db.execute("delete from auth.users where id = $1::text::uuid", &[&user])
        .unwrap();

    let left: i64 = db
        .query_one(
            "select (select count(*) from sync_log where account_id = $1::text::uuid)
                  + (select count(*) from sync_snapshots where account_id = $1::text::uuid)",
            &[&user],
        )
        .unwrap()
        .get(0);
    assert_eq!(left, 0);
}
