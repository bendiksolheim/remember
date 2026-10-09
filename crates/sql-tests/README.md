# sql-tests

Tests for the `sync_*` functions in
[`supabase/schema.sql`](../../supabase/schema.sql) against a real Postgres.
The tests live in `tests/sync_functions.rs`; `src/lib.rs` is empty.

Every test is `#[ignore]`d, so `cargo xtask test` skips them. Run them with:

```bash
cargo xtask pgtest
```

That needs macOS and Apple's `container` CLI (with `container system start`
done). It starts a throwaway `postgres:17` container, then applies
`supabase/test/supabase_shim.sql`, `schema.sql` twice (to prove it re-runs
cleanly) and `lockdown.sql`. It runs the ignored tests, and removes the
container whatever happens.
