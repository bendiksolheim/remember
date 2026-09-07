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
