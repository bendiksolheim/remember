# Sync backend setup (Supabase)

This is the recipe for standing up the backend that device sync (`crates/sync`,
`todo-sync`) talks to. Sync is opt-in — the app works fully with none of this
done — so there's no rush to do this until you actually want to test syncing
across devices.

## 1. Create a project

1. [supabase.com](https://supabase.com) → New project. Free tier is enough
   for personal use — this schema is one small table plus auth, nothing that
   needs a paid plan.
2. Pick a region close to you; it doesn't matter functionally, only for
   latency.

## 2. Apply the schema

SQL Editor → New query → paste the contents of [`schema.sql`](schema.sql) →
Run. It creates `sync_log` (the append-only update-blob log) and
`sync_snapshots` (for devices joining late — see the comments in the file
for why), with row-level security scoping every row to `auth.uid()`.

Re-running it is safe (every statement is `if not exists`/`or replace`
guarded) if the schema ever needs to change and you want to reapply.

## 3. Enable auth providers

Authentication → Providers:

- **Email** is on by default — sign-up/sign-in with email+password works
  with no further setup, and this is what `AuthClient::sign_up`/`sign_in`
  in `todo-sync` use.
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
  no path suffix. `todo-sync`'s `HttpTransport`/`AuthClient` append
  `/rest/v1/...` and `/auth/v1/...` themselves — passing a URL that
  already includes `/rest/v1/` double-prefixes every request and 404s.
- **`anon`/`publishable` key** (safe to embed in a client — RLS is what
  actually protects the data, not keeping this key secret). Supabase has
  two key formats depending on when the project was created (legacy JWT
  `anon` key, or the newer `sb_publishable_...` key) — either works here,
  it's just passed straight through as the `apikey` header.

These are the two values `SyncClient::new(supabaseUrl:anonKey:)`
(`crates/ffi/src/lib.rs`) takes, surfaced as `TodoModel`'s `supabaseURL`/
`supabaseAnonKey` init parameters.

## 5. Wire them into the app

Currently hardcoded at the `TodoModel(...)` call site in
`swift/Sources/TodoMac/AppDelegate.swift` — there's no secret-management
story yet (no `.xcconfig`, no keychain bootstrap, nothing read from
environment). Since the anon/publishable key is meant to be public (see
step 4), hardcoding it in a tracked source file is a reasonable stopgap for
a personal build, not a security problem — just not a polished one. Worth
revisiting (env var, `.xcconfig`, or a settings field in the app itself)
before this is ever a build someone other than you runs.

Once wired, the status-bar menu ("Sync…") opens a small window
(`SyncSettingsView`/`SettingsWindowController` in `swift/Sources/TodoMac/`)
to sign up/in and trigger a manual sync — that's the only UI surface for
sync right now, deliberately minimal.

## Notes for later

- **Compaction**: `sync_snapshots` exists in the schema but nothing writes
  to it yet — no scheduled job folds old `sync_log` rows into a snapshot.
  Fine to leave until the log is actually large enough to matter for a
  late-joining device.
- **Encryption**: the server can read task content in plaintext (see the
  design discussion this schema came out of) — deliberate for v1, not an
  oversight.
- **This has never been tested against a real project** — the Rust side
  (`crates/sync`) is tested against a local mock HTTP server
  (`wiremock`), not this actual schema. The first real run against a live
  Supabase project is the thing most likely to surface a mismatch (e.g. in
  how `bytea` round-trips through PostgREST's JSON encoding) — see
  `crates/sync/src/transport.rs`'s hex-encoding comments if push/pull ever
  errors in a way that looks like a payload-encoding problem.
