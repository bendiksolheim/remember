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

        // Derived from a fresh random UUID rather than adding a `rand`
        // dependency solely for one random u64.
        let id = Uuid::new_v4().as_u64_pair().0;
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
        let bytes: Option<Vec<u8>> = self
            .lock()
            .query_row(
                "SELECT value FROM meta WHERE key = 'pulled_seq'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_err)?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let arr: [u8; 8] = bytes
            .try_into()
            .map_err(|_| CoreError::Storage("corrupt pulled_seq in meta table".to_string()))?;
        Ok(Some(i64::from_le_bytes(arr)))
    }

    pub fn save_pulled_seq(&self, seq: i64) -> Result<(), CoreError> {
        self.lock()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('pulled_seq', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (seq.to_le_bytes().to_vec(),),
            )
            .map_err(store_err)?;
        Ok(())
    }

    /// The sidebar/"current" list selected on *this device* — not synced,
    /// since different devices may legitimately be looking at different
    /// lists at the same time. `None` means "never set" (fresh install) or
    /// explicitly "All lists".
    pub fn load_current_list(&self) -> Result<Option<String>, CoreError> {
        self.load_meta_string("current_list")
    }

    pub fn save_current_list(&self, list_id: Option<&str>) -> Result<(), CoreError> {
        self.save_meta_string("current_list", list_id)
    }

    /// The last *concrete* list a capture landed in on this device —
    /// distinct from [`Store::load_current_list`], which can be "All": a
    /// new task always needs one real destination list, even while the
    /// current view is showing every list combined, so this tracks
    /// whichever concrete list was selected most recently regardless of
    /// whether the view has since moved to "All". `None` means never set.
    pub fn load_capture_list(&self) -> Result<Option<String>, CoreError> {
        self.load_meta_string("capture_list")
    }

    pub fn save_capture_list(&self, list_id: &str) -> Result<(), CoreError> {
        self.save_meta_string("capture_list", Some(list_id))
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
