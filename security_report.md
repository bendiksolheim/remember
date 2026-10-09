# Security review — Rust core + macOS app

Date: 2026-10-04. Scope: `crates/core`, `crates/sync`, `crates/ffi`, `swift/Sources/TodoKit`,
`swift/Sources/TodoMac`, `supabase/schema.sql`, release pipeline (`.github/workflows`,
`Casks/todo.rb`, `crates/xtask`). The iOS app was out of scope.

Method: manual code review. Finding 1 was confirmed with a proof of concept; everything
else is from reading the code. The Swift code could not be compiled or run in the review
environment.

| # | Severity | Finding | Status |
|---|----------|---------|--------|
| 1 | High | Signing out can silently undo itself | Fixed |
| 2 | Medium | Switching sync accounts under one macOS login mixes data (integrity) | Fixed |
| 3 | Medium | Shipping switches off macOS protections (ad-hoc signing, no hardened runtime, quarantine stripped) | Open |
| 4 | Medium | Release builds `main`, not the tag; tag name injected into shell | Fixed |
| 5 | Medium | Sign-out doesn't revoke the session server-side | Fixed |
| 6 | Medium | Two independent copies of the session fight over refresh-token rotation | Fixed |
| 7 | Low | Deleted todos persist in CRDT history; no end-to-end encryption | Open |
| 8 | Low | RLS policies broader than needed; no payload size limit | Open |
| 9 | Low | Keychain handling hygiene | Fixed (data protection keychain deferred to 3) |
| 10 | Low | HTTP client allows http and follows redirects | Fixed |
| 11 | Info | Login item without consent; panel visible in screen sharing; global `seq` | Fixed (`seq` accepted) |

---

## High

### 1. Signing out can silently undo itself

`crates/sync/src/coordinator.rs` (`State::attempt_sync`), `swift/Sources/TodoKit/Model.swift` (`signOut`).

The background sync clones the session, does its network round-trip, then runs:

```rust
if lock(&self.session).as_ref() != Some(&session) {   // None != Some(..) → true
    *lock(&self.session) = Some(session.clone());
    on_refreshed(session);                             // → Swift persist() → Keychain
}
```

If the user signs out while a sync is in flight, the stored session is `None`, so the
check passes even though no refresh happened. The old session is put back in the
coordinator (auto-sync keeps running) and written back to the Keychain. The UI says
"signed out", but the next launch is signed in again. If a different sync account signs
in during that window, the old account's session can overwrite the new one.

The manual "Sync Now" path (`SyncClient::sync_now` in `crates/ffi`) has the same
problem whenever a refresh happens during the in-flight sync.

**Proof of concept:** a transport whose pull sleeps 500 ms, start a sync, call
`set_session(None)` mid-flight. Result: `keychain after sign-out: Some("user-A-refresh")`.

**Fix:** only write back a session if the stored one is still the session the sync
started with (compare-and-swap); never persist a refreshed session after sign-out.

**Resolution (2026-10-04):**
- `AutoSyncCoordinator` now adopts a refreshed session only via a compare-and-swap
  (`adopt_refreshed`) against the session the round started with; otherwise it is
  neither stored nor reported.
- The manual `SyncClient::sync_now` path (FFI) reports a refresh only if the
  coordinator adopts it. This also partly addresses finding 6: a manual refresh now
  updates the coordinator's copy too.
- Swift: refresh callbacks go through `persistRefreshed`, which does nothing once
  `isSignedIn` is false, closing the window between Rust's check and the main-actor hop.
- Tests: `signing_out_mid_sync_is_not_undone_when_the_sync_finishes`,
  `a_refresh_that_races_a_sign_out_is_not_reported`,
  `adopt_refreshed_replaces_the_session_only_if_it_is_still_the_original`. Both race
  tests fail against the old code.

## Medium

### 2. Switching sync accounts under one macOS login mixes data

`crates/core/src/store.rs` (meta keys `pushed_vv`, `pulled_seq`), `Model.swift` (`signOut`).

**Scope:** the database (`~/Library/Application Support/no.bendik.todo/todo.sqlite3`)
and the Keychain session are per macOS user, so separate macOS logins on the same Mac
never share them. This only applies when two different *sync accounts* are used under
the *same* macOS login: one person with e.g. a personal and a work account, or people
sharing one macOS login (who can already read each other's files). No one gains access
they didn't already have, so this was originally rated High as a "shared Mac" leak and
has been downgraded: it is primarily a data-integrity bug.

Sign-out deliberately keeps local data, but the sync cursors are not tied to an account.
When account B signs in after account A under the same macOS login:

- A's todos stay in the local document and show up under B. A's already-synced todos
  aren't re-uploaded (A's `pushed_vv` covers them), but A's not-yet-pushed edits, and
  any edits made to A's todos while signed in as B, are pushed into B's account, where
  B's other devices may be unable to apply them (they reference history B never got).
- B's server rows with `seq` ≤ A's cursor are skipped (`seq` is a single global identity
  across all users), so B's own data can silently fail to arrive.
- B's rows that are pulled merge into the same document. If A later signs back in and
  edits, the next push is computed from A's old `pushed_vv`, so B's todos can be
  uploaded into A's account.

**Fix:** record which account the local sync state belongs to, and reset (or separate)
local state when a different account signs in.

**Resolution (2026-10-04):** chose "reset local".
- New `App::bind_sync_account(user_id)` in `todo-core`, called by `SyncEngine` at the
  start of every round:
  - First account ever: bound, and local data is kept (todos made before signing in are
    uploaded, as before). Existing installs upgrade into this case.
  - Same account: no-op.
  - Different account: in one SQLite transaction, an empty document, a **fresh peer id**
    (so the new account's existing ops from this device can't collide with new ones),
    and cleared `pushed_vv`/`pulled_seq`/`current_list`. The previous account's unpushed
    edits on this device are lost; its synced data stays on the server.
- `Shared::flush_now` now holds the state lock across the save, so a debounced flush
  can't write the previous account's document over the reset.
- `SyncEngine` re-checks the bound account before each import and before pushing, and
  abandons the round (`SyncError::Auth`) if it changed mid-flight.
- Tests: five in `crates/core/tests/sync.rs` (including a failed reset being all-or-nothing), three in `crates/sync/src/engine.rs`. All
  three engine tests fail with the binding/checks removed.
- Residual: the mid-round check and the import are not under one lock, so a reset
  landing in that microsecond window is not caught. In practice it would require
  signing out, signing in as someone else, and finishing that account's first round
  within one in-flight request.

### 3. Shipping switches off macOS protections

`crates/xtask/src/main.rs` (`mac`: `codesign --force --sign -`), `Casks/todo.rb` (`postflight xattr -cr`).

- Ad-hoc signed only: no Developer ID, no notarization, no hardened runtime.
- The Cask strips the quarantine attribute, bypassing Gatekeeper entirely.
- Without the hardened runtime, any same-user process can launch Todo with
  `DYLD_INSERT_LIBRARIES` and read the Keychain token from inside the trusted process,
  with no prompt.
- Ad-hoc signatures change every build, so each update triggers a Keychain access
  prompt. Users learn to click "Always Allow", and a swapped-in malicious `Todo.app`
  gets the same familiar-looking prompt.

**Fix:** Developer ID signing with `--options runtime`, notarize, drop the `xattr` step.

### 4. Release builds `main`, not the tag

`.github/workflows/release.yml`.

- `actions/checkout` uses `ref: main`, so a release labelled `vN` contains whatever
  `main` holds when the job runs.
- `${{ steps.version.outputs.version }}` (derived from the tag name) is interpolated
  directly into `run:` scripts and a `sed` expression — script injection pattern
  (requires write access to create the tag, so low exploitability).
- Actions not pinned to commit SHAs; `contents: write` granted workflow-wide.

**Fix:** check out `github.event.release.tag_name`; pass the version via `env:` and
validate it (e.g. `^[0-9]+(\.[0-9]+)*$`); pin actions by SHA.

**Resolution (2026-10-06):**
- The build job checks out `refs/tags/<tag>`, not `main`. Only the job that commits the
  Cask checks out `main`.
- The tag is validated as `^v[0-9]+(\.[0-9]+)*$` before anything uses it. The version,
  tag, zip name and sha256 reach `run:` scripts only through `env:`; none is interpolated
  into the scripts. The sha256 is also checked as 64 hex characters before the `sed`.
- Split into two jobs. `build` (macOS) has `contents: read` and `persist-credentials: false`,
  so dependency build scripts can't use a write token. `publish` (Ubuntu) has
  `contents: write`, uploads the release asset and commits the Cask bump.
- All actions are pinned by commit SHA (`checkout` v4.4.0, `upload-artifact` v4.6.2,
  `download-artifact` v4.3.0), in `ci.yml` too.
- Checked with actionlint only; the workflow has not been run.

### 5. Sign-out doesn't revoke the session server-side

`Model.swift` (`signOut`) only deletes the Keychain item. Nothing calls
`POST /auth/v1/logout`. A refresh token stolen earlier stays valid after sign-out, and
Supabase refresh tokens don't expire by default.

**Fix:** call the logout endpoint (best effort) on sign-out.

**Resolution (2026-10-06):**
- New `AuthClient::sign_out` in `todo-sync` calls `POST /auth/v1/logout?scope=local`.
  The scope matters: Supabase's default is `global`, which revokes *every* session the
  user has and would sign out their other devices too. `local` revokes only the session
  the access token belongs to (each sign-in is its own session), together with all of
  its refresh tokens, including any rotated by a sync that raced the sign-out.
- The endpoint needs a valid access token, so an expired one is refreshed first. A
  rejected refresh token (400/401/403/404) or a logout answered with 401/403/404 means
  the session is already gone and counts as done. Other failures (network, 5xx, a 400
  from logout) are errors.
- `todo_sync::sign_out` atomically takes the auto-sync coordinator's session
  (`AutoSyncCoordinator::take_session`, which also pauses auto-sync). If the coordinator
  has none, it falls back to the caller's stored copy. It then revokes on a background
  thread. The coordinator's copy is preferred because it may be newer (finding 6).
- FFI `SyncClient::sign_out(session)` delegates to it. Swift `signOut` deletes the
  Keychain item and clears `isSignedIn` first, then calls it, so signing out never
  depends on the network.
- Accepted residual: best effort. If the device is offline or the app quits right
  away, the session stays valid on the server; there is no retry.
- Tests: eight in `auth.rs` (including `sign_out_revokes_only_this_devices_session`,
  which asserts `scope=local` and fails without it) and five in `coordinator.rs`.
  The Swift change and the regenerated bindings have not been built yet, and nothing
  has been tested against a real Supabase project.

### 6. Two independent copies of the session

`Model.swift` (`syncNow`) refreshes from the Keychain copy and persists the result, but
never updates the auto-sync coordinator's copy. Supabase invalidates a refresh token as
soon as it's rotated, so the coordinator later refreshes with a revoked token. Supabase
treats that as token reuse and may revoke the whole session family — the user gets
logged out and sees unexplained sync errors.

**Fix:** a single owner for the session (the coordinator); route "Sync Now" through it or
call `setSyncToken` after every refresh.

**Resolution (2026-10-06):** the fix for finding 1 already made a manual refresh update
the coordinator, but only once the whole round had finished. Until then both copies held
a refresh token Supabase had already revoked. A second round started in that window
refreshed with it: "Sync Now" during an auto-sync round, the other way round, or the
auto-sync triggered by the Sync Now round's own import. A slow round (over the 10 s
reuse window) got the session revoked.
- The coordinator is the only owner and the only place a session is refreshed. "Sync
  Now" no longer runs a round of its own: `SyncClient::sync_now` and
  `SyncEngine::sync_now` are gone, and Swift's `syncNow` calls `syncSoon()`. Every round
  runs on the coordinator's one thread, so two refreshes can never overlap.
- `SyncEngine` is split into `refresh` and `sync_once`. `attempt_sync` adopts a rotated
  session (compare-and-swap, as in finding 1) and reports it for persisting straight
  after the refresh, before the pull and push. If the user signed out while the refresh
  was in flight, the round stops there: nothing is adopted, reported, pulled or pushed.
- Swift reads the Keychain only at launch and sign-out. The result of "Sync Now" arrives
  through the auto-sync listener, which now also clears `isSyncing`. If a round was
  already running, the spinner stops when that round ends and the requested round runs
  right after it.
- Tests (`coordinator.rs`): `sync_soon_during_a_round_never_refreshes_the_session_twice`,
  `a_rotated_session_is_adopted_and_reported_before_the_round_goes_on`,
  `signing_out_during_the_refresh_abandons_the_round`,
  `a_rotated_session_is_kept_even_when_the_round_then_fails`,
  `a_failed_refresh_is_reported_and_keeps_the_session_without_syncing`. The second and
  third fail if adoption is moved back to the end of the round. The first passes
  against the old coordinator too: the old race went through the separate FFI path,
  which no longer exists. It guards against adding a second refresh path again.
- Residual: Swift persists the reported session after a hop to the main actor. A crash
  in that gap (now milliseconds rather than a whole round) leaves the Keychain with a
  revoked token. A failed Keychain write is finding 9. The Swift change has not been
  built.

## Low

### 7. Deleted todos aren't really deleted; no end-to-end encryption

The Loro update log retains inserted content even after deletion, so deleted text stays
in `sync_log` on the server and in the local snapshot. Payloads are not encrypted
client-side, so anyone with database access (Supabase operator, a leaked service-role
key) can read every todo ever written. At minimum, document this for users.

### 8. RLS policies broader than needed

`supabase/schema.sql`. `for all` lets a stolen access token update/delete `sync_log`
rows (destroying history for new devices) and inject arbitrary CRDT updates — e.g. a
fake "Call IT support at …" todo, a phishing vector into the user's own list. Sign-up
is open and there is no payload size limit (storage abuse).

**Fix:** `select` + `insert` policies only; a `check (octet_length(payload) < N)`
constraint; consider rate limiting / email confirmation.

### 9. Keychain handling

`swift/Sources/TodoKit/KeychainStore.swift`.

- Uses the legacy file-based keychain (`kSecUseDataProtectionKeychain` not set); no
  explicit `kSecAttrAccessible`.
- `save` deletes then adds, ignoring `SecItemAdd`'s status. If the add fails, the
  session is lost — unrecoverable, since refresh tokens rotate.
- The Cask's `zap` doesn't remove the Keychain item.

**Fix:** `SecItemUpdate` falling back to `SecItemAdd`, check statuses, use the data
protection keychain with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`.

**Resolution (2026-10-07):**
- `KeychainStore.save` calls `SecItemUpdate` and falls back to `SecItemAdd` only on
  `errSecItemNotFound`. It never deletes first, so a failed write leaves the previous
  session in place. `save` and `delete` throw `KeychainError(status)` on any other status
  (`delete` treats "not found" as success).
- `TodoModel` handles the failures:
  - Sign-in/sign-up: if the session can't be stored, the call throws (the error shows in
    the sign-in form), the user stays signed out, and the new session is revoked so it
    isn't left live on the server.
  - A rotated session that can't be stored sets `keychainError`. Sync keeps running on the
    coordinator's in-memory copy, but the next launch won't have it.
  - A failed delete on sign-out sets `keychainError` and still signs out. The revoke from
    finding 5 makes the leftover token useless.
  - `keychainError` is kept separate from `lastSyncError`, which every successful round
    clears. `SyncSettingsView` shows it in both the signed-in and signed-out states.
- iOS: items are stored with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`. Updates
  set it too, so an existing item moves over on its next refresh. Not set on macOS: the
  legacy keychain has no accessibility classes.
- Cask `zap` runs `security delete-generic-password -s no.bendik.todo.sync -a session`
  (`must_succeed: false`, since there is no item if the user never signed in).
- **Deferred to finding 3:** the data protection keychain on macOS. It requires a
  `keychain-access-groups` entitlement, which an ad-hoc signed build can't carry, so
  turning it on now would most likely break every Keychain call with
  `errSecMissingEntitlement` (-34018). When Developer ID signing lands: add the
  entitlement, set `kSecUseDataProtectionKeychain` and the accessibility class on macOS
  too, migrate the legacy item (read it, write it to the new keychain, delete the old
  one), and change the Cask's `zap`, since `security` can't see data protection items.
- Not built or run: no Swift toolchain in the review environment. The Cask hasn't been
  checked by `brew audit` or even Ruby.

### 10. HTTP client allows http and follows redirects

`crates/sync/src/auth.rs`, `crates/sync/src/transport.rs`. `reqwest::blocking::Client::new()`
doesn't enforce `https_only`, and follows redirects; on a 307/308 the email/password
body is re-sent to the redirect target. The URL is hard-coded to https today, so this
is defence in depth.

**Fix:** `Client::builder().https_only(true).redirect(Policy::none())`.

**Resolution (2026-10-07):**
- `AuthClient` and `HttpTransport` both get their client from one `http_client()` in
  `crates/sync/src/lib.rs`: `https_only(true)` and `redirect::Policy::none()`. A 3xx now
  comes back as an ordinary non-success response, so it surfaces as an auth or transport
  error instead of being followed.
- `https_only` is off in `cfg(test)` builds only, because the unit tests talk to
  wiremock over plain http. The redirect policy applies in tests too.
- Tests (`auth.rs`): `a_redirect_is_not_followed_and_the_credentials_stay_put` (a 307 to a
  second server that must receive nothing) and `the_production_client_refuses_plain_http`
  (the production client never reaches an http server). Each fails with its half of the
  hardening removed.

### 11. Informational

- The app registers itself as a login item without asking (`AppDelegate.swift`,
  `registerLoginItemIfNeeded`).
- The capture panel joins all Spaces and appears in screen sharing/recordings
  (`sharingType` not set) — a hotkey press during a screen share exposes todos.
- `sync_log.seq` is global, so any user can infer total service activity.

**Resolution (2026-10-07):**
- Login item: `registerLoginItemIfNeeded` is gone. Opening at login is a toggle in the
  redesigned Settings window (General tab), off unless the user turns it on. Its state is
  read back from `SMAppService.mainApp.status` rather than stored, so a change in System
  Settings shows up too. `requiresApproval` shows a hint and a button that opens Login
  Items in System Settings. Installs that the old code already registered stay
  registered; the toggle shows them as on, and the user can turn it off.
- Screen sharing: the capture panel sets `sharingType = .none`, so it is left out of
  screen sharing, screenshots and recordings. No setting for it.
- Global `seq`: accepted. It reveals only how many sync rows exist across the service,
  not whose they are or what they contain.
- The settings window was rebuilt alongside this (General · Lists · Account tabs, fixed
  size, in the Dock and ⌘Tab while open; plan in `settings.md`). Related changes:
  `Session` carries the account's email so Account can show who is signed in; the core
  persists `last_synced_at` (cleared on an account switch); `ListRow.task_count` drives a
  confirmation before deleting a list that has todos; Sign Out warns that signing in to a
  different account later replaces the todos on this Mac (finding 2); `cargo xtask
  package` writes the release version into Info.plist before signing.
- Rust: tested (`cargo xtask test`, clippy, no new uncovered lines). Swift: not built or
  run, no Swift toolchain in the review environment.

## Non-security bug found along the way

- **Edits made during a push can be lost from sync.** `App::mark_pushed` records the
  document's version vector *at the time it's called*, not at the time of
  `export_for_push`. An edit made while the push request is on the wire is covered by
  the recorded vector but was never in the pushed bytes, so it is never pushed. Fix:
  have `export_for_push` return the vector it exported up to, and pass that to
  `mark_pushed`.

## Checked and found fine

- No SQL injection: all SQLite queries are parameterized.
- `todo-core` denies `unwrap`/`expect`/`panic`; extreme synced `due` values
  (`i64::MIN`/`MAX`) were run through a release build without panicking.
- Synced titles are rendered with `Text(String)` (verbatim), not Markdown — no link
  injection.
- Global hotkey uses Carbon `RegisterEventHotKey`: no Accessibility permission, no key
  observation.
- Error messages surfaced to the UI don't contain tokens.
- TLS via rustls; RLS is enabled; `account_id` defaults to `auth.uid()`.
- The Supabase publishable key in the binary is public by design.
