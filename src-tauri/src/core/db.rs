//! Persistent storage: the hash cache now, the undo journal in Phase 4.
//!
//! Only things that must survive a crash live here. The scan index does not -
//! a stale snapshot of someone's filesystem is a safety hazard, because it
//! invites the app to act on files that have since changed.
//!
//! **The cache is keyed on file identity, not on path.** A rename or a move -
//! including one this app performs - must not throw away the cost of hashing a
//! 4 GB file. A cache hit is trusted only when size *and* modification time
//! both still match, so editing a file in place correctly invalidates it.

use crate::core::model::FileIdent;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

const SCHEMA_VERSION: i64 = 1;

pub struct Db {
    /// `rusqlite::Connection` is `Send` but not `Sync`. Rather than fight that
    /// with a pool, all database work is batched: the dedupe pipeline reads the
    /// whole cache it needs in one transaction, hashes in parallel with no
    /// database access at all, then writes results back in one transaction.
    /// Contention is therefore two brief locks per run, not one per file.
    conn: Mutex<Connection>,
}

/// What identifies a cached hash, and what must still be true to trust it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub ident: FileIdent,
    pub size: u64,
    pub mtime_ns: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct CacheEntry {
    pub key: CacheKey,
    pub hash: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct CachedThumb {
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::prepare(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    /// Used by tests, and as the fallback when the database file cannot be
    /// opened: the app stays fully usable, it just re-hashes next time.
    pub fn open_in_memory() -> rusqlite::Result<Db> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    fn prepare(conn: &Connection) -> rusqlite::Result<()> {
        // WAL so a read never blocks the write at the end of a run.
        // `synchronous = FULL` because the write volume here is trivial next to
        // the cost of losing a journal record in Phase 4 - correctness over
        // throughput is the right trade for data we cannot reconstruct.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Self::migrate(conn)
    }

    fn migrate(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
              key   TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );

            -- volume_id and file_id hold u64 values reinterpreted as i64,
            -- because SQLite has no unsigned type. They are opaque keys, never
            -- compared for ordering, so the wrap is harmless.
            CREATE TABLE IF NOT EXISTS hash_cache (
              volume_id   INTEGER NOT NULL,
              file_id     INTEGER NOT NULL,
              size        INTEGER NOT NULL,
              mtime_ns    INTEGER NOT NULL,
              full_hash   BLOB NOT NULL,
              computed_at INTEGER NOT NULL,
              PRIMARY KEY (volume_id, file_id)
            ) WITHOUT ROWID;

            -- The undo journal. Rows are committed BEFORE the first
            -- filesystem call, so a crash mid-batch still leaves a durable
            -- record of exactly what was attempted.
            CREATE TABLE IF NOT EXISTS operations (
              id         INTEGER PRIMARY KEY,
              kind       TEXT NOT NULL,
              root       BLOB NOT NULL,
              created_at INTEGER NOT NULL,
              state      TEXT NOT NULL,
              note       TEXT NOT NULL DEFAULT ''
            );

            -- Paths are BLOBs holding the platform's native encoding (UTF-16
            -- on Windows, bytes on Unix), not TEXT. A filename is not
            -- guaranteed to be valid UTF-8, and undo has to reproduce the
            -- original path exactly - a lossy round-trip would make some
            -- files impossible to restore.
            CREATE TABLE IF NOT EXISTS operation_items (
              id           INTEGER PRIMARY KEY,
              operation_id INTEGER NOT NULL REFERENCES operations(id) ON DELETE CASCADE,
              from_path    BLOB NOT NULL,
              to_path      BLOB,
              size         INTEGER NOT NULL DEFAULT 0,
              state        TEXT NOT NULL,
              error        TEXT
            );

            -- Thumbnails, keyed on identity exactly like hash_cache: a rename
            -- must not throw away a decode. `max_px` is part of the key so
            -- changing the preview size does not serve stale sizes.
            CREATE TABLE IF NOT EXISTS thumb_cache (
              volume_id   INTEGER NOT NULL,
              file_id     INTEGER NOT NULL,
              max_px      INTEGER NOT NULL,
              size        INTEGER NOT NULL,
              mtime_ns    INTEGER NOT NULL,
              mime        TEXT NOT NULL,
              width       INTEGER NOT NULL,
              height      INTEGER NOT NULL,
              data        BLOB NOT NULL,
              computed_at INTEGER NOT NULL,
              PRIMARY KEY (volume_id, file_id, max_px)
            ) WITHOUT ROWID;

            -- Swipe Clean decisions. Keyed on identity alone, not size or
            -- mtime: a decision is about the file, and renaming or editing it
            -- does not mean you want to be asked about it again.
            CREATE TABLE IF NOT EXISTS review_state (
              volume_id  INTEGER NOT NULL,
              file_id    INTEGER NOT NULL,
              decision   TEXT NOT NULL,
              decided_at INTEGER NOT NULL,
              PRIMARY KEY (volume_id, file_id)
            ) WITHOUT ROWID;

            CREATE INDEX IF NOT EXISTS idx_items_op ON operation_items(operation_id);
            CREATE INDEX IF NOT EXISTS idx_ops_created ON operations(created_at DESC);
            "#,
        )?;

        let found: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| {
                row.get(0)
            })
            .optional()?;

        if found.is_none() {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
        Ok(())
    }

    /// Run a closure with the connection. Used by `journal`, which owns the
    /// operations schema; the lock stays private so there is exactly one place
    /// that decides how the database is shared.
    pub(crate) fn with_conn<R>(&self, f: impl FnOnce(&mut Connection) -> R) -> R {
        let mut conn = self.conn.lock();
        f(&mut conn)
    }

    /// Look up many keys in one transaction.
    ///
    /// A row whose `size` or `mtime_ns` no longer matches is treated as a miss
    /// and left for `store` to overwrite.
    pub fn lookup(&self, keys: &[CacheKey]) -> Vec<Option<[u8; 32]>> {
        let mut results = vec![None; keys.len()];
        if keys.is_empty() {
            return results;
        }

        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare_cached(
            "SELECT size, mtime_ns, full_hash FROM hash_cache \
             WHERE volume_id = ?1 AND file_id = ?2",
        ) else {
            return results;
        };

        for (slot, key) in results.iter_mut().zip(keys) {
            let row = stmt
                .query_row(
                    (key.ident.volume as i64, key.ident.file_id as i64),
                    |row| {
                        let size: i64 = row.get(0)?;
                        let mtime_ns: i64 = row.get(1)?;
                        let hash: Vec<u8> = row.get(2)?;
                        Ok((size, mtime_ns, hash))
                    },
                )
                .optional();

            if let Ok(Some((size, mtime_ns, hash))) = row {
                // Both must match. Trusting a hash whose file has changed is
                // how a tool ends up telling someone two different files are
                // the same.
                if size as u64 == key.size && mtime_ns == key.mtime_ns && hash.len() == 32 {
                    let mut out = [0u8; 32];
                    out.copy_from_slice(&hash);
                    *slot = Some(out);
                }
            }
        }

        results
    }

    /// Write computed hashes back in one transaction. Returns rows written.
    pub fn store(&self, entries: &[CacheEntry]) -> rusqlite::Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let now = crate::core::model::now_unix_secs();
        let mut written = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO hash_cache \
                   (volume_id, file_id, size, mtime_ns, full_hash, computed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(volume_id, file_id) DO UPDATE SET \
                   size = excluded.size, \
                   mtime_ns = excluded.mtime_ns, \
                   full_hash = excluded.full_hash, \
                   computed_at = excluded.computed_at",
            )?;
            for entry in entries {
                written += stmt.execute((
                    entry.key.ident.volume as i64,
                    entry.key.ident.file_id as i64,
                    entry.key.size as i64,
                    entry.key.mtime_ns,
                    entry.hash.as_slice(),
                    now,
                ))?;
            }
        }
        tx.commit()?;
        Ok(written)
    }

    /// A cached thumbnail, if one exists for this exact file and size.
    ///
    /// Same rule as the hash cache: a hit is trusted only when size and
    /// modification time both still match, so editing an image in place
    /// correctly invalidates its preview.
    pub fn thumb_get(&self, key: &CacheKey, max_px: u32) -> Option<CachedThumb> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT size, mtime_ns, mime, width, height, data FROM thumb_cache \
                 WHERE volume_id = ?1 AND file_id = ?2 AND max_px = ?3",
            )
            .ok()?;
        let row = stmt
            .query_row(
                (key.ident.volume as i64, key.ident.file_id as i64, max_px),
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .optional()
            .ok()??;

        let (size, mtime_ns, mime, width, height, data) = row;
        if size as u64 != key.size || mtime_ns != key.mtime_ns {
            return None;
        }
        Some(CachedThumb {
            mime,
            width: width.max(0) as u32,
            height: height.max(0) as u32,
            data,
        })
    }

    pub fn thumb_put(
        &self,
        key: &CacheKey,
        max_px: u32,
        thumb: &CachedThumb,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO thumb_cache \
               (volume_id, file_id, max_px, size, mtime_ns, mime, width, height, data, computed_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) \
             ON CONFLICT(volume_id, file_id, max_px) DO UPDATE SET \
               size = excluded.size, mtime_ns = excluded.mtime_ns, mime = excluded.mime, \
               width = excluded.width, height = excluded.height, data = excluded.data, \
               computed_at = excluded.computed_at",
            rusqlite::params![
                key.ident.volume as i64,
                key.ident.file_id as i64,
                max_px,
                key.size as i64,
                key.mtime_ns,
                thumb.mime,
                thumb.width,
                thumb.height,
                thumb.data,
                crate::core::model::now_unix_secs(),
            ],
        )?;
        Ok(())
    }

    /// Rows and total bytes held by cached previews, for Settings.
    pub fn thumb_usage(&self) -> (u64, u64) {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(LENGTH(data)), 0) FROM thumb_cache",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map(|(count, bytes)| (count.max(0) as u64, bytes.max(0) as u64))
        .unwrap_or((0, 0))
    }

    pub fn thumb_clear(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM thumb_cache", [])
    }

    /// Every recorded review decision, keyed by file identity.
    pub fn review_all(&self) -> std::collections::HashMap<(u64, u64), String> {
        let conn = self.conn.lock();
        let mut out = std::collections::HashMap::new();
        let Ok(mut stmt) = conn.prepare("SELECT volume_id, file_id, decision FROM review_state")
        else {
            return out;
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, String>(2)?,
            ))
        }) else {
            return out;
        };
        for (volume, file, decision) in rows.flatten() {
            out.insert((volume, file), decision);
        }
        out
    }

    pub fn review_set(&self, ident: FileIdent, decision: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_state (volume_id, file_id, decision, decided_at) \
             VALUES (?1,?2,?3,?4) \
             ON CONFLICT(volume_id, file_id) DO UPDATE SET \
               decision = excluded.decision, decided_at = excluded.decided_at",
            rusqlite::params![
                ident.volume as i64,
                ident.file_id as i64,
                decision,
                crate::core::model::now_unix_secs()
            ],
        )?;
        Ok(())
    }

    /// Forget one decision, so undo puts a file back in the queue.
    pub fn review_forget(&self, ident: FileIdent) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM review_state WHERE volume_id = ?1 AND file_id = ?2",
            rusqlite::params![ident.volume as i64, ident.file_id as i64],
        )?;
        Ok(())
    }

    pub fn review_clear(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM review_state", [])
    }

    pub fn cached_count(&self) -> u64 {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM hash_cache", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|n| n.max(0) as u64)
        .unwrap_or(0)
    }

    /// Drop cache rows older than `days`, so an abandoned volume's hashes do
    /// not accumulate forever.
    pub fn prune(&self, days: i64) -> rusqlite::Result<usize> {
        let cutoff = crate::core::model::now_unix_secs() - days.max(1) * 86_400;
        let conn = self.conn.lock();
        conn.execute("DELETE FROM hash_cache WHERE computed_at < ?1", [cutoff])
    }

    pub fn clear(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM hash_cache", [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(file_id: u64, size: u64, mtime_ns: i64) -> CacheKey {
        CacheKey {
            ident: FileIdent {
                volume: 42,
                file_id,
            },
            size,
            mtime_ns,
        }
    }

    #[test]
    fn stores_and_retrieves_a_hash() {
        let db = Db::open_in_memory().unwrap();
        let k = key(1, 100, 5_000);
        let hash = [7u8; 32];

        assert_eq!(db.lookup(&[k]), vec![None], "nothing cached yet");

        db.store(&[CacheEntry { key: k, hash }]).unwrap();
        assert_eq!(db.lookup(&[k]), vec![Some(hash)]);
        assert_eq!(db.cached_count(), 1);
    }

    #[test]
    fn a_changed_size_or_mtime_invalidates_the_entry() {
        let db = Db::open_in_memory().unwrap();
        let k = key(2, 100, 5_000);
        db.store(&[CacheEntry {
            key: k,
            hash: [1u8; 32],
        }])
        .unwrap();

        // Same file, edited in place: the cached hash must not be trusted.
        assert_eq!(db.lookup(&[key(2, 100, 9_999)]), vec![None], "mtime changed");
        assert_eq!(db.lookup(&[key(2, 250, 5_000)]), vec![None], "size changed");
        // Unchanged still hits.
        assert_eq!(db.lookup(&[k]), vec![Some([1u8; 32])]);
    }

    #[test]
    fn re_storing_the_same_identity_updates_rather_than_duplicates() {
        let db = Db::open_in_memory().unwrap();
        db.store(&[CacheEntry {
            key: key(3, 10, 1),
            hash: [1u8; 32],
        }])
        .unwrap();
        db.store(&[CacheEntry {
            key: key(3, 20, 2),
            hash: [2u8; 32],
        }])
        .unwrap();

        assert_eq!(db.cached_count(), 1, "identity is the primary key");
        assert_eq!(db.lookup(&[key(3, 20, 2)]), vec![Some([2u8; 32])]);
    }

    #[test]
    fn lookup_preserves_order_and_handles_mixed_hits() {
        let db = Db::open_in_memory().unwrap();
        db.store(&[CacheEntry {
            key: key(10, 1, 1),
            hash: [9u8; 32],
        }])
        .unwrap();

        let results = db.lookup(&[key(99, 1, 1), key(10, 1, 1), key(98, 1, 1)]);
        assert_eq!(results, vec![None, Some([9u8; 32]), None]);
    }

    #[test]
    fn empty_lookup_and_store_are_no_ops() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.lookup(&[]).is_empty());
        assert_eq!(db.store(&[]).unwrap(), 0);
    }

    #[test]
    fn large_identity_values_round_trip_through_signed_storage() {
        // u64 values above i64::MAX are reinterpreted, not clamped. A real
        // Windows volume serial plus file index can land here.
        let db = Db::open_in_memory().unwrap();
        let k = CacheKey {
            ident: FileIdent {
                volume: u64::MAX,
                file_id: u64::MAX - 1,
            },
            size: 1,
            mtime_ns: 1,
        };
        db.store(&[CacheEntry {
            key: k,
            hash: [3u8; 32],
        }])
        .unwrap();
        assert_eq!(db.lookup(&[k]), vec![Some([3u8; 32])]);
    }

    #[test]
    fn survives_being_reopened_from_disk() {
        let path = std::env::temp_dir().join(format!(
            "cleanup-db-{}.sqlite",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let k = key(5, 77, 1234);
        {
            let db = Db::open(&path).unwrap();
            db.store(&[CacheEntry {
                key: k,
                hash: [4u8; 32],
            }])
            .unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.lookup(&[k]), vec![Some([4u8; 32])]);
        }

        // WAL leaves sidecar files behind.
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn clear_empties_the_cache() {
        let db = Db::open_in_memory().unwrap();
        db.store(&[CacheEntry {
            key: key(1, 1, 1),
            hash: [0u8; 32],
        }])
        .unwrap();
        assert_eq!(db.cached_count(), 1);
        db.clear().unwrap();
        assert_eq!(db.cached_count(), 0);
    }
}
