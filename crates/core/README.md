# remember-core

All domain logic. Everything the app decides — what a task is, sorting,
filtering, date formatting, due-date parsing, undo/redo, the sync contract —
lives here. Frontends only render what this crate hands them.

## Key files

| File | What it does |
|---|---|
| `src/doc.rs` | The [Loro](https://loro.dev) CRDT document. The **only** file that may mention Loro |
| `src/session.rs` | `Session`: the document plus what a client is looking at (view filter, current list) and the rules tying them together. No I/O, no threads |
| `src/app.rs` | `App`: wraps `Session` with storage and a debounced background writer. The type the Swift frontend talks to (via `remember-ffi`) |
| `src/store.rs` | SQLite persistence of the document's snapshot bytes |
| `src/command.rs` | `Command`, everything a frontend can ask for |
| `src/snapshot.rs` | `Snapshot`, what frontends render: already filtered, sorted and formatted |
| `src/clock.rs` | Injected time and identity. `SystemTime::now()` and `Uuid::new_v4()` must never appear outside this module |
| `src/civil.rs` | Calendar-day arithmetic, no date-library dependency |
| `src/due_parse.rs` | Detects trailing due-date phrases ("buy milk tomorrow") in typed text |

## Features

- `native` (default) — SQLite, `Store` and `App`. Turned off for the wasm
  build (`crates/web`), which only uses `Session`.
- `testing` — deterministic clock/id helpers for tests outside the crate.

## Persistence

- macOS: `~/Library/Application Support/no.bendik.todo/todo.sqlite3`
- iOS: the app container's own Application Support directory

The SQLite file holds exactly two things: the latest exported Loro snapshot
(`doc` table) and a random per-install peer id (`meta` table, generated once,
stable for the life of the install — never share a peer id across devices).
There are no normalized task tables; the CRDT document is the source of
truth, and the whole list comfortably fits in memory for any real personal
task list.

The `no.bendik.todo` path is kept from the app's original name on purpose —
see [`swift/README.md`](../../swift/README.md#apple).

## Rules

- 100% line coverage. See [`TESTING.md`](../../TESTING.md).
- `unsafe` is forbidden, and `unwrap`/`expect`/`panic` are denied by lint.
