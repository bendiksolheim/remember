-- Run once every device runs an app version that syncs through the
-- `sync_push`/`sync_pull`/`sync_compact` functions in `schema.sql`, after
-- applying that file. Safe to re-run.
--
-- Revokes direct table access from clients. Until this runs, an older
-- client still reads `sync_log` directly and silently misses every row a
-- compaction deleted; afterwards it gets an HTTP error instead. It also
-- stops any client from inserting around the per-account lock `sync_push`
-- takes. The functions are `security definer`, so they're unaffected.

revoke all on table sync_log, sync_snapshots from anon, authenticated;
