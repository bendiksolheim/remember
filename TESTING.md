# Testing

`cargo xtask test` runs the whole workspace through `cargo nextest`;
`cargo xtask ci` adds `fmt --check`, `clippy -D warnings` and the coverage
gate. Both are plain Rust and run on any platform.

## Suites

`remember-core` (`crates/core/tests/`):

- `integration.rs` — the behavioral suite, driving the core through commands.
- `properties.rs` — proptest. Property 1, convergence of independently-edited
  CRDT replicas merged in either order, is the most important test in the
  project.
- `snapshots.rs` — `insta` snapshots, reviewed by hand, not just accepted.
- `persistence.rs` — SQLite round-trips, using `tempfile` — never a real
  user path.
- `due_detection.rs` — due-date phrase parsing.
- `sync.rs` — the core's sync primitives.

`remember-sync` and `remember-ffi` have unit tests in their own `src/`.

## Coverage

`cargo xtask cov` runs `cargo llvm-cov` over `remember-core`, `remember-sync`
and `remember-ffi`, fails under 100% line coverage, and opens the HTML
report. The only exclusion is `crates/ffi/src/lib.rs`, which is pure
delegation; `crates/ffi/src/convert.rs` holds all the real conversion logic
and is covered.

Read the HTML report occasionally rather than just checking the exit code —
it's how you catch a line that ran but was never actually asserted against.

Tests that need deterministic time or ids use the `remember-core/testing`
feature (`crates/core/src/clock.rs`), which `test`, `cov` and `ci` all enable.

## Mutation testing

```bash
cargo mutants -p remember-core --features remember-core/testing
```

Not part of the CI gate (slow). Worth rerunning whenever `doc.rs` or the
date-formatting code changes substantially.

## SQL functions

`cargo xtask pgtest` (macOS only) tests `supabase/schema.sql` against a real
Postgres. See [`crates/sql-tests/README.md`](crates/sql-tests/README.md).

## Swift

Swift has no automated tests by design. If a Swift bug ever turns out to be a
logic bug, that logic was in the wrong language: move it to `remember-core`
and cover it there.
