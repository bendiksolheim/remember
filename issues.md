# Issues to file

Seven issues, in suggested priority order. Each has a title, labels and a body
to paste as-is. Labels used: `bug`, `security`, `sync`, `data-loss`, `release`,
`enhancement`.

---

## 4. No payload size limit or rate limiting; sign-up is open

**Labels:** `security`, `sync`

Anyone can sign up and then call `sync_push`/`sync_compact` with arbitrarily large
payloads, as often as they like. That costs database storage and egress, which
matters more once sync is a paid feature.

### Fix

- `check (octet_length(payload) < N)` on `sync_log.payload`, and a larger limit on
  `sync_snapshots.payload`. Real sizes for reference: about 25 KB per 100 todos and
  about 250 KB per 1000, with an extreme account (20 000 todos ever created) at
  about 4.3 MB. Snapshots grow with history, so their limit needs headroom.
- Email confirmation on sign-up; consider Supabase's rate limits for auth.
- Once sync is paid: only accounts with an active subscription may push.

The direct-table half of finding 8 in `security_report.md` is fixed by
`supabase/lockdown.sql`; this is what remains.

---

## 6. Deleted todos persist in sync history; no end-to-end encryption

**Labels:** `security`, `sync`, `enhancement`

### What happens

- The Loro document keeps the full history, so a deleted todo's text stays in the
  local database, in every device's copy, and in the server snapshot.
  Measured: each deleted todo still costs about 120 bytes even in a history-free
  (shallow) snapshot, and much more with history.
- Payloads aren't encrypted on the client, so anyone with database access
  (the Supabase operator, a leaked service-role key) can read every todo ever written.

### Options

- Document it for users, at minimum.
- Shallow snapshots at compaction time drop history. That needs care: a device that
  was offline across the cut may not be able to merge.
- Client-side encryption of payloads. Compaction already happens on the client, so
  the server never needs to read them.

Source: finding 7 in `security_report.md`.
