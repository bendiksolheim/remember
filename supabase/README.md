# Sync backend setup (Supabase)

This is the recipe for standing up the backend that device sync (`crates/sync`,
`remember-sync`) talks to. Sync is opt-in — the app works fully with none of this
done — so there's no rush to do this until you actually want to test syncing
across devices.

## 1. Create a project

1. [supabase.com](https://supabase.com) → New project. Free tier is enough
   for personal use — this schema is two small tables plus auth, nothing that
   needs a paid plan.
2. Pick a region close to you; it doesn't matter functionally, only for
   latency.

## 2. Apply the schema

SQL Editor → New query → paste the contents of [`schema.sql`](schema.sql) →
Run. It creates `sync_log` (the append-only update-blob log),
`sync_snapshots` (one full-document snapshot per account that replaces the
log rows it covers), and the `sync_push`/`sync_pull`/`sync_compact`
functions clients call instead of touching the tables. See the comments in
the file for why.

Re-running it is safe (every statement is `if not exists`/`or replace`
guarded) if the schema ever needs to change and you want to reapply.

Then paste and run [`lockdown.sql`](lockdown.sql), which revokes direct
table access from clients. On a project that already has devices syncing,
update the app on **every** device first: an older app reads `sync_log`
directly and silently misses rows once compaction deletes them. With the
lockdown in place it gets an HTTP error instead.

### Testing the schema

`cargo xtask pgtest` (macOS, needs Apple's `container` CLI with
`container system start` done) runs `crates/sql-tests` against a throwaway
`postgres:17` container. It applies [`test/supabase_shim.sql`](test/supabase_shim.sql)
(a stand-in for Supabase's `auth.uid()` and client roles), then
`schema.sql` twice and `lockdown.sql`, runs the tests, and removes the
container.

## 3. Enable auth providers

Authentication → Providers:

- **Email** is on by default — sign-up/sign-in with email+password works
  with no further setup, and this is what `AuthClient::sign_up`/`sign_in`
  in `remember-sync` use.
- **Apple** ("Sign in with Apple") if you want it as an option — this is a
  separate identity provider from iCloud and does **not** require the
  device be logged into iCloud, which is exactly why it was chosen over
  CloudKit/iCloud-based sync in the first place. Needs a Services ID +
  key from your Apple Developer account; Supabase's own
  [Apple provider guide](https://supabase.com/docs/guides/auth/social-login/auth-apple)
  covers that part.
- Anything else (Google, GitHub, ...) is optional and not specifically
  planned for — add only if you want it.

## 4. Get your project's URL and anon key

Project Settings → API:

- **Project URL** — the *bare* project URL (`https://<ref>.supabase.co`),
  no path suffix. `remember-sync`'s `HttpTransport`/`AuthClient` append
  `/rest/v1/...` and `/auth/v1/...` themselves — passing a URL that
  already includes `/rest/v1/` double-prefixes every request and 404s.
- **`anon`/`publishable` key** (safe to embed in a client — RLS is what
  actually protects the data, not keeping this key secret). Supabase has
  two key formats depending on when the project was created (legacy JWT
  `anon` key, or the newer `sb_publishable_...` key) — either works here,
  it's just passed straight through as the `apikey` header.

These are the two values `SyncClient::new(supabaseUrl:anonKey:)`
(`crates/ffi/src/lib.rs`) takes, surfaced as `RememberModel`'s `supabaseURL`/
`supabaseAnonKey` init parameters.

## 5. Wire them into the app

Currently hardcoded at the `RememberModel(...)` call site in
`swift/Sources/RememberMac/AppDelegate.swift` — there's no secret-management
story yet (no `.xcconfig`, no keychain bootstrap, nothing read from
environment). Since the anon/publishable key is meant to be public (see
step 4), hardcoding it in a tracked source file is a reasonable stopgap for
a personal build, not a security problem — just not a polished one. Worth
revisiting (env var, `.xcconfig`, or a settings field in the app itself)
before this is ever a build someone other than you runs.

Once wired, the status-bar menu ("Sync…") opens a small window
(`SyncSettingsView`/`SettingsWindowController` in `swift/Sources/RememberMac/`)
to sign up/in and trigger a manual sync — that's the only UI surface for
sync right now, deliberately minimal.

## Notes for later

- **Compaction**: clients do it. After a round that finds more than 200 log
  rows past the snapshot, the device uploads its whole document as the new
  snapshot and the rows it covers are deleted (`SyncEngine::compact`), so
  every account stays at one snapshot plus a short tail. The snapshot keeps
  the full Loro history, so it grows with how much an account has ever
  done (roughly 0.2 KB per task ever created), not with time or sync
  frequency.
- **Encryption**: the server can read task content in plaintext (see the
  design discussion this schema came out of) — deliberate for v1, not an
  oversight.
- **Not tested end to end** — the SQL is tested against real Postgres
  (`cargo xtask pgtest`) and the Rust side (`crates/sync`) against a local
  mock HTTP server (`wiremock`), but never the two together through
  PostgREST. The first run of the `sync_*` functions against a live project
  is the thing most likely to surface a mismatch (e.g. in how `bytea`
  arguments and `jsonb` results round-trip through PostgREST) — see
  `crates/sync/src/transport.rs`'s hex-encoding comments if push/pull ever
  errors in a way that looks like a payload-encoding problem.
