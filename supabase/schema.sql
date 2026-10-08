-- Sync backend schema for the task app.
--
-- Run this once in a fresh Supabase project (SQL Editor → New query → paste
-- → Run). Safe to re-run: every statement is idempotent.
--
-- Storage model: an append-only log of Loro CRDT update-blobs per account
-- (`sync_log`), plus one full-document snapshot per account
-- (`sync_snapshots`) that subsumes every log row up to `as_of_seq`. Clients
-- compact: once the log tail grows past a threshold, a caught-up device
-- uploads its full document as the new snapshot and the rows it covers are
-- deleted. So each account holds one snapshot row plus a short tail, however
-- long it has been in use. Nothing here does any merge logic — Loro's own
-- import is commutative and idempotent, so this is purely "store bytes, hand
-- them back in order." See `crates/sync` (`remember-sync`) for the Rust side.
--
-- Clients only ever go through the three `sync_*` functions below, never
-- the tables: the functions take a per-account lock and read consistently,
-- which plain PostgREST table access can't. `lockdown.sql` revokes direct
-- table access once every client is on a version that uses them.

create table if not exists sync_log (
  seq        bigint generated always as identity primary key,
  -- `default auth.uid()`, not just `not null`: an older client inserts
  -- straight into this table without an `account_id`, so this default is
  -- what populates the column, filled in from the JWT PostgREST already
  -- validated for this request. `sync_push` sets it explicitly.
  account_id uuid not null default auth.uid() references auth.users (id) on delete cascade,
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
  account_id uuid primary key default auth.uid() references auth.users (id) on delete cascade,
  payload    bytea not null,
  -- The `sync_log.seq` this snapshot subsumes: a device that has this
  -- snapshot plus every `sync_log` row with `seq > as_of_seq` has the full
  -- history. Rows with `seq <= as_of_seq` no longer exist.
  as_of_seq  bigint not null,
  updated_at timestamptz not null default now()
);

-- Fixes an already-created table from a previous run of this file (the
-- `create table if not exists` above won't retroactively add a default to
-- an existing column) — safe to run even on a table that already has it.
alter table sync_log alter column account_id set default auth.uid();
alter table sync_snapshots alter column account_id set default auth.uid();

-- Same for `on delete cascade`: deleting a user in Supabase removes their
-- log and snapshot instead of failing. Their devices keep their local tasks.
alter table sync_log
  drop constraint if exists sync_log_account_id_fkey,
  add constraint sync_log_account_id_fkey
    foreign key (account_id) references auth.users (id) on delete cascade;
alter table sync_snapshots
  drop constraint if exists sync_snapshots_account_id_fkey,
  add constraint sync_snapshots_account_id_fkey
    foreign key (account_id) references auth.users (id) on delete cascade;

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

-- The functions are `security definer` so they keep working after
-- `lockdown.sql` revokes table access from clients. That bypasses the RLS
-- policies above, so every statement scopes itself to `auth.uid()`
-- explicitly, and each function refuses to run without one.

-- Serialises `sync_push` and `sync_compact` per account. `seq` is assigned
-- when an insert runs, not when it commits, so without this two devices
-- pushing at once could commit 102 before 101, and a pull in between would
-- move its cursor past 101 for good. Holding the lock until commit makes
-- seq order equal commit order within an account. Other accounts aren't
-- blocked (barring a hash collision, which only costs a short wait).
create or replace function sync_lock_account() returns uuid
language plpgsql security definer set search_path = '' as $$
declare
  uid uuid := auth.uid();
begin
  if uid is null then
    raise exception 'not authenticated' using errcode = '28000';
  end if;
  perform pg_advisory_xact_lock(hashtextextended('sync_log:' || uid::text, 0));
  return uid;
end
$$;

-- Appends one update-blob, returning its seq.
create or replace function sync_push(device_id bigint, payload bytea) returns bigint
language plpgsql security definer set search_path = '' as $$
declare
  uid uuid := public.sync_lock_account();
  new_seq bigint;
begin
  insert into public.sync_log (account_id, device_id, payload)
  values (uid, sync_push.device_id, sync_push.payload)
  returning seq into new_seq;
  return new_seq;
end
$$;

-- Everything a device with cursor `since` (null: never pulled) is missing:
--
--   { "snapshot": { "payload": "\x..", "as_of_seq": n } | null,
--     "rows":     [ { "seq": n, "payload": "\x.." }, ... ],
--     "has_more": bool,
--     "log_len":  n }
--
-- `snapshot` is present only when `since` is behind it, since the rows
-- between `since` and `as_of_seq` may already be gone. `rows` follow
-- whichever of the two is later, oldest first, at most 1000 per call;
-- `has_more` says to call again with the new cursor. `log_len` is how many
-- rows the log holds past the snapshot, for the client's decision to
-- compact. One statement, so all of it comes from one database snapshot: a
-- compaction can't land between reading the snapshot and reading the rows.
create or replace function sync_pull(since bigint) returns jsonb
language plpgsql stable security definer set search_path = '' as $$
declare
  uid uuid := auth.uid();
  result jsonb;
begin
  if uid is null then
    raise exception 'not authenticated' using errcode = '28000';
  end if;

  with snap as (
    select s.payload, s.as_of_seq
    from public.sync_snapshots s
    where s.account_id = uid
  ), page as (
    select l.seq, l.payload
    from public.sync_log l
    where l.account_id = uid
      and l.seq > greatest(coalesce(since, -1), coalesce((select as_of_seq from snap), -1))
    order by l.seq
    limit 1001
  )
  select jsonb_build_object(
    'snapshot', (
      select jsonb_build_object('payload', snap.payload, 'as_of_seq', snap.as_of_seq)
      from snap
      where since is null or since < snap.as_of_seq
    ),
    'rows', coalesce((
      select jsonb_agg(jsonb_build_object('seq', p.seq, 'payload', p.payload) order by p.seq)
      from (select * from page order by page.seq limit 1000) p
    ), '[]'::jsonb),
    'has_more', (select count(*) from page) > 1000,
    'log_len', (
      select count(*)
      from public.sync_log l
      where l.account_id = uid
        and l.seq > coalesce((select as_of_seq from snap), -1)
    )
  ) into result;

  return result;
end
$$;

-- Replaces the account's snapshot with `payload`, which must contain every
-- log row up to and including `as_of_seq`, and deletes those rows. Returns
-- false, changing nothing, unless `as_of_seq` is one of the account's
-- current rows: that rejects a stale compaction (its rows are already
-- folded into a newer snapshot) and a seq that isn't this account's.
create or replace function sync_compact(payload bytea, as_of_seq bigint) returns boolean
language plpgsql security definer set search_path = '' as $$
declare
  uid uuid := public.sync_lock_account();
begin
  if not exists (
    select 1 from public.sync_log l
    where l.account_id = uid and l.seq = sync_compact.as_of_seq
  ) then
    return false;
  end if;

  insert into public.sync_snapshots (account_id, payload, as_of_seq, updated_at)
  values (uid, sync_compact.payload, sync_compact.as_of_seq, now())
  on conflict (account_id) do update
    set payload = excluded.payload,
        as_of_seq = excluded.as_of_seq,
        updated_at = excluded.updated_at;

  delete from public.sync_log l
  where l.account_id = uid and l.seq <= sync_compact.as_of_seq;

  return true;
end
$$;

-- Postgres grants execute to everyone by default, and Supabase grants it to
-- `anon` explicitly too. Only signed-in users get to call these.
revoke execute on function sync_lock_account() from public, anon, authenticated;
revoke execute on function sync_push(bigint, bytea) from public, anon;
revoke execute on function sync_pull(bigint) from public, anon;
revoke execute on function sync_compact(bytea, bigint) from public, anon;
grant execute on function sync_push(bigint, bytea) to authenticated;
grant execute on function sync_pull(bigint) to authenticated;
grant execute on function sync_compact(bytea, bigint) to authenticated;

-- Makes PostgREST pick up new or changed functions straight away.
notify pgrst, 'reload schema';
