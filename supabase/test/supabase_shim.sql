-- Test only: the slice of a Supabase project that `schema.sql` relies on,
-- so it runs unchanged on a plain `postgres` image. `cargo xtask pgtest`
-- applies this first, then `schema.sql` (twice, to prove it re-runs) and
-- `lockdown.sql`. Never run this against a real Supabase project.

do $$
begin
  if not exists (select 1 from pg_roles where rolname = 'anon') then
    create role anon nologin;
  end if;
  if not exists (select 1 from pg_roles where rolname = 'authenticated') then
    create role authenticated nologin;
  end if;
end
$$;

create schema if not exists auth;
create table if not exists auth.users (id uuid primary key default gen_random_uuid());

-- Same as Supabase's: the `sub` claim of the JWT PostgREST validated,
-- which it exposes to SQL as the `request.jwt.claims` setting. Tests set
-- that themselves with `set_config('request.jwt.claims', ..., true)`.
create or replace function auth.uid() returns uuid
language sql stable as $$
  select nullif(
    nullif(current_setting('request.jwt.claims', true), '')::jsonb ->> 'sub',
    ''
  )::uuid
$$;

grant usage on schema auth to anon, authenticated;
grant usage on schema public to anon, authenticated;

-- Supabase grants clients everything on new tables in `public` (RLS is what
-- actually restricts them), which is what `lockdown.sql` revokes, and
-- execute on every new function, which `schema.sql` revokes where needed.
alter default privileges in schema public grant all on tables to anon, authenticated;
alter default privileges in schema public grant execute on functions to anon, authenticated;
