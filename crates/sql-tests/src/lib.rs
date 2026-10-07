//! Tests for `supabase/schema.sql`'s `sync_*` functions against a real
//! Postgres, in `tests/`. Every test is `#[ignore]`d so `cargo xtask test`
//! skips them: `cargo xtask pgtest` starts a throwaway Postgres container,
//! applies the schema, and runs them. Nothing to see in this file.
