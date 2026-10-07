//! SQLite persistence. The Loro document is the source of truth; SQLite only
//! provides crash-safe storage for its exported snapshot bytes, plus a
//! per-install peer id. No normalized task tables — the whole document fits
//! in memory for any realistic personal todo list.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::CoreError;

const SCHEMA_SQL: &str = "
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS doc (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  snapshot   BLOB    NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value BLOB NOT NULL
);
";

fn store_err<E: std::fmt::Display>(e: E) -> CoreError {
    CoreError::Storage(e.to_string())
}

/// Derived from a fresh random UUID rather than adding a `rand` dependency
/// solely for one random u64.
pub(crate) fn fresh_peer_id() -> u64 {
    Uuid::new_v4().as_u64_pair().0
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self, CoreError> {
        let conn = Connection::open(path).map_err(store_err)?;
        conn.execute_batch(SCHEMA_SQL).map_err(store_err)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Stable for the life of the install, unique per device. Read from
    /// `meta`, or generated and persisted on first access.
    pub fn peer_id(&self) -> Result<u64, CoreError> {
        let conn = self.lock();
        let existing: Option<Vec<u8>> = conn
            .query_row("SELECT value FROM meta WHERE key = 'peer_id'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(store_err)?;

        if let Some(bytes) = existing {
            let arr: [u8; 8] = bytes
                .try_into()
                .map_err(|_| CoreError::Storage("corrupt peer_id in meta table".to_string()))?;
            return Ok(u64::from_le_bytes(arr));
        }

        let id = fresh_peer_id();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('peer_id', ?1)",
            (id.to_le_bytes().to_vec(),),
        )
        .map_err(store_err)?;
        Ok(id)
    }

    /// The Loro version vector (encoded) as of the last successful sync
    /// push, i.e. "what the server already has from this device". `None`
    /// means this device has never pushed — the next push should export the
    /// full local history.
    pub fn load_pushed_vv(&self) -> Result<Option<Vec<u8>>, CoreError> {
        self.lock()
            .query_row(
                "SELECT value FROM meta WHERE key = 'pushed_vv'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_err)
    }

    pub fn save_pushed_vv(&self, vv: &[u8]) -> Result<(), CoreError> {
        self.lock()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('pushed_vv', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (vv,),
            )
            .map_err(store_err)?;
        Ok(())
    }

    /// The server's log cursor as of the last successful pull, i.e. "the
    /// highest `seq` this device has already imported". `None` means this
    /// device has never pulled — the next pull should start from the
    /// beginning of the account's log.
    pub fn load_pulled_seq(&self) -> Result<Option<i64>, CoreError> {
        self.load_meta_i64("pulled_seq")
    }

    pub fn save_pulled_seq(&self, seq: i64) -> Result<(), CoreError> {
        self.save_meta_i64("pulled_seq", seq)
    }

    /// When this device last finished a sync round without errors, in Unix
    /// seconds. `None` means never (or not since a different account took
    /// over, see `reset_for_account`).
    pub fn load_last_synced_at(&self) -> Result<Option<i64>, CoreError> {
        self.load_meta_i64("last_synced_at")
    }

    pub fn save_last_synced_at(&self, at: i64) -> Result<(), CoreError> {
        self.save_meta_i64("last_synced_at", at)
    }

    /// The list selected on *this device* — both what's displayed and where
    /// new captures land. Not synced, since different devices may
    /// legitimately be looking at different lists at the same time. `None`
    /// means "never set" (fresh install).
    pub fn load_current_list(&self) -> Result<Option<String>, CoreError> {
        self.load_meta_string("current_list")
    }

    pub fn save_current_list(&self, list_id: &str) -> Result<(), CoreError> {
        self.save_meta_string("current_list", Some(list_id))
    }

    /// The sync account (user id) this install's local data and sync
    /// cursors belong to. `None` means no account has ever synced here.
    pub fn load_sync_account(&self) -> Result<Option<String>, CoreError> {
        self.load_meta_string("sync_account")
    }

    pub fn save_sync_account(&self, account: &str) -> Result<(), CoreError> {
        self.save_meta_string("sync_account", Some(account))
    }

    /// Hands this install over to a different sync account, atomically:
    /// replaces the document with `snapshot` (a fresh, empty one) under a
    /// new `peer_id`, forgets both sync cursors, the last sync time and the current list, and
    /// records `account` as the owner. All-or-nothing, so a crash midway
    /// can never leave one account's data bound to another.
    pub fn reset_for_account(
        &self,
        account: &str,
        peer_id: u64,
        snapshot: &[u8],
        updated_at: i64,
    ) -> Result<(), CoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(store_err)?;
        tx.execute(
            "DELETE FROM meta WHERE key IN ('pushed_vv', 'pulled_seq', 'last_synced_at', 'current_list')",
            [],
        )
        .map_err(store_err)?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('peer_id', ?1), ('sync_account', ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (peer_id.to_le_bytes().to_vec(), account.as_bytes()),
        )
        .map_err(store_err)?;
        tx.execute(
            "INSERT INTO doc (id, snapshot, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET snapshot = excluded.snapshot, updated_at = excluded.updated_at",
            (snapshot, updated_at),
        )
        .map_err(store_err)?;
        tx.commit().map_err(store_err)
    }

    fn load_meta_i64(&self, key: &str) -> Result<Option<i64>, CoreError> {
        let bytes: Option<Vec<u8>> = self
            .lock()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(store_err)?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let arr: [u8; 8] = bytes
            .try_into()
            .map_err(|_| CoreError::Storage(format!("corrupt {key} in meta table")))?;
        Ok(Some(i64::from_le_bytes(arr)))
    }

    fn save_meta_i64(&self, key: &str, value: i64) -> Result<(), CoreError> {
        self.lock()
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value.to_le_bytes().to_vec()),
            )
            .map_err(store_err)?;
        Ok(())
    }

    fn load_meta_string(&self, key: &str) -> Result<Option<String>, CoreError> {
        let bytes: Option<Vec<u8>> = self
            .lock()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(store_err)?;
        bytes
            .map(|b| String::from_utf8(b).map_err(|e| store_err(e.to_string())))
            .transpose()
    }

    /// `None` deletes the key (rather than storing an empty value) so
    /// `load_meta_string` unambiguously reports "never set" afterward.
    fn save_meta_string(&self, key: &str, value: Option<&str>) -> Result<(), CoreError> {
        let conn = self.lock();
        match value {
            Some(v) => conn
                .execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    (key, v.as_bytes()),
                )
                .map(|_| ())
                .map_err(store_err),
            None => conn
                .execute("DELETE FROM meta WHERE key = ?1", [key])
                .map(|_| ())
                .map_err(store_err),
        }
    }

    pub fn load_snapshot(&self) -> Result<Option<Vec<u8>>, CoreError> {
        self.lock()
            .query_row("SELECT snapshot FROM doc WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(store_err)
    }

    pub fn save_snapshot(&self, snapshot: &[u8], updated_at: i64) -> Result<(), CoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(store_err)?;
        tx.execute(
            "INSERT INTO doc (id, snapshot, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET snapshot = excluded.snapshot, updated_at = excluded.updated_at",
            (snapshot, updated_at),
        )
        .map_err(store_err)?;
        tx.commit().map_err(store_err)
    }
}
