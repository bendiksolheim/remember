# remember-sync

Device sync: pushes and pulls Loro update blobs to and from a Supabase project
over `reqwest::blocking`. Opt-in — the app works fully with no backend
configured.

The core's document already merges commutatively and idempotently, so this
crate has no merge logic of its own. Its only job is getting bytes to and
from a server and knowing when to call `App`'s sync primitives.

| File | What it does |
|---|---|
| `src/transport.rs` | `SyncTransport`: push/pull/compact over opaque bytes. `HttpTransport` talks to Supabase; `InMemoryTransport` is a test stand-in following the server's rules |
| `src/engine.rs` | `SyncEngine`: session refresh and the pull → merge → push round. The one place that decides the order of `App`'s sync primitives |
| `src/coordinator.rs` | When to sync: local edits settling, a periodic fallback, and explicit "sync soon" for lifecycle events |
| `src/auth.rs` | `AuthClient`, a thin wrapper over Supabase's GoTrue REST API. Persisting the session (Keychain) is the caller's job |

The backend this talks to is described in
[`supabase/README.md`](../../supabase/README.md), with tables and RLS in
[`supabase/schema.sql`](../../supabase/schema.sql).
