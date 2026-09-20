-- Sync backend schema for the todo app.
--
-- Run this once in a fresh Supabase project (SQL Editor → New query → paste
-- → Run). Safe to re-run: every statement is idempotent.
--
-- Storage model: an append-only log of Loro CRDT update-blobs per account
-- (`sync_log`), plus a periodic full-state snapshot (`sync_snapshots`) so a
-- device joining late doesn't have to replay the whole log since account
-- creation. Nothing here does any merge logic — Loro's own import is
-- commutative and idempotent, so this is purely "store bytes, hand them
-- back in order." See `crates/sync` (`todo-sync`) for the Rust side that
-- talks to this schema, and the "why" behind these choices in the project
-- memory / design notes from the sync architecture discussion.

create table if not exists sync_log (
  seq        bigint generated always as identity primary key,
  -- `default auth.uid()`, not just `not null`: the Rust client
  -- (`HttpTransport::push`) never sends `account_id` in the insert body at
  -- all — it only knows the bearer token, not its own account id — so this
  -- default is what actually populates the column, filled in from the JWT
  -- PostgREST already validated for this request. Without it, every insert
  -- gets `account_id = NULL`, which then fails the RLS policy below rather
  -- than a more obvious NOT NULL error.
  account_id uuid not null default auth.uid() references auth.users (id),
  -- The pushing device's `App::peer_id` (a `u64`), stored via
  -- `peer_id as i64` bit-reinterpretation on the Rust side — Postgres has
  -- no unsigned 64-bit type. Convert back with `as u64` on read.
  device_id  bigint not null,
  payload    bytea not null,
  created_at timestamptz not null default now()
);

create index if not exists sync_log_account_seq on sync_log (account_id, seq);

create table if not exists sync_snapshots (
  -- Same reasoning as `sync_log.account_id` above.
  account_id uuid primary key default auth.uid() references auth.users (id),
  payload    bytea not null,
  -- The `sync_log.seq` this snapshot subsumes: a device that has this
  -- snapshot plus every `sync_log` row with `seq > as_of_seq` has the full
  -- history without replaying everything from the start.
  as_of_seq  bigint not null,
  updated_at timestamptz not null default now()
);

-- Fixes an already-created table from a previous run of this file (the
-- `create table if not exists` above won't retroactively add a default to
-- an existing column) — safe to run even on a table that already has it.
alter table sync_log alter column account_id set default auth.uid();
alter table sync_snapshots alter column account_id set default auth.uid();

alter table sync_log enable row level security;
alter table sync_snapshots enable row level security;

drop policy if exists "own rows only" on sync_log;
create policy "own rows only" on sync_log
  for all
  using (account_id = auth.uid())
  with check (account_id = auth.uid());

drop policy if exists "own snapshot only" on sync_snapshots;
create policy "own snapshot only" on sync_snapshots
  for all
  using (account_id = auth.uid())
  with check (account_id = auth.uid());

-- Not built yet, deliberately: a scheduled job that folds old `sync_log`
-- rows into `sync_snapshots` once every known device has seen them, so the
-- log doesn't grow forever. The schema supports adding this later without a
-- migration; skip it until the log size is actually a problem.
