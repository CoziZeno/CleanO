//! The operation journal.
//!
//! The rule that makes this worth having: **every item is committed to disk
//! before the first filesystem call.** A crash halfway through a batch
//! therefore leaves a durable, accurate record of what was attempted, which is
//! what makes recovery on next launch possible rather than guesswork.
//!
//! Paths are stored as BLOBs in the platform's native encoding, not TEXT.
//! Filenames are not guaranteed to be valid UTF-8, and undo has to reproduce
//! the original path *exactly*; a lossy round-trip would make some files
//! impossible to restore - precisely the files a careless tool would lose.

use crate::core::db::Db;
use crate::core::model::now_unix_secs;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OpKind {
    /// Files moved into category folders.
    Organize,
    /// Files sent to the Recycle Bin / Trash.
    Trash,
}

impl OpKind {
    pub fn key(self) -> &'static str {
        match self {
            OpKind::Organize => "organize",
            OpKind::Trash => "trash",
        }
    }
    fn parse(value: &str) -> OpKind {
        match value {
            "trash" => OpKind::Trash,
            _ => OpKind::Organize,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OpState {
    /// Written, nothing attempted yet. Seeing this at startup means we crashed.
    Planned,
    Running,
    Done,
    /// Some items succeeded, some did not. A first-class outcome, not an error.
    Partial,
    Failed,
    Undone,
}

impl OpState {
    pub fn key(self) -> &'static str {
        match self {
            OpState::Planned => "planned",
            OpState::Running => "running",
            OpState::Done => "done",
            OpState::Partial => "partial",
            OpState::Failed => "failed",
            OpState::Undone => "undone",
        }
    }
    fn parse(value: &str) -> OpState {
        match value {
            "running" => OpState::Running,
            "done" => OpState::Done,
            "partial" => OpState::Partial,
            "failed" => OpState::Failed,
            "undone" => OpState::Undone,
            _ => OpState::Planned,
        }
    }
    /// Did this operation change the filesystem in a way undo can reverse?
    pub fn is_reversible(self) -> bool {
        matches!(self, OpState::Done | OpState::Partial)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Pending,
    Done,
    Failed,
    Reverted,
}

impl ItemState {
    fn key(self) -> &'static str {
        match self {
            ItemState::Pending => "pending",
            ItemState::Done => "done",
            ItemState::Failed => "failed",
            ItemState::Reverted => "reverted",
        }
    }
    fn parse(value: &str) -> ItemState {
        match value {
            "done" => ItemState::Done,
            "failed" => ItemState::Failed,
            "reverted" => ItemState::Reverted,
            _ => ItemState::Pending,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlannedItem {
    pub from: PathBuf,
    /// Where it is intended to go. `None` for a trash operation.
    pub to: Option<PathBuf>,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct ItemRecord {
    pub id: i64,
    pub from: PathBuf,
    /// The path actually used, which may differ from the plan when a
    /// collision forced a rename.
    pub to: Option<PathBuf>,
    pub size: u64,
    pub state: ItemState,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OperationRecord {
    pub id: i64,
    pub kind: OpKind,
    pub root: PathBuf,
    pub created_at: i64,
    pub state: OpState,
    pub note: String,
    pub items: Vec<ItemRecord>,
}

impl OperationRecord {
    pub fn done_count(&self) -> usize {
        self.items.iter().filter(|i| i.state == ItemState::Done).count()
    }
    pub fn failed_count(&self) -> usize {
        self.items.iter().filter(|i| i.state == ItemState::Failed).count()
    }
    pub fn bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.state == ItemState::Done)
            .map(|i| i.size)
            .sum()
    }
}

/// A journalled operation that has been committed and is ready to execute.
pub struct Begun {
    pub operation_id: i64,
    /// Row ids in the same order as the items passed to `begin`.
    pub item_ids: Vec<i64>,
}

/// Write the whole plan to disk in one transaction, in state `planned`.
///
/// Nothing has happened on the filesystem when this returns - that is the
/// point. If the process dies immediately afterwards, the next launch finds a
/// `planned` operation and knows nothing was attempted.
pub fn begin(
    db: &Db,
    kind: OpKind,
    root: &Path,
    note: &str,
    items: &[PlannedItem],
) -> rusqlite::Result<Begun> {
    db.with_conn(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO operations (kind, root, created_at, state, note) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![
                kind.key(),
                path_to_blob(root),
                now_unix_secs(),
                OpState::Planned.key(),
                note
            ],
        )?;
        let operation_id = tx.last_insert_rowid();

        let mut item_ids = Vec::with_capacity(items.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO operation_items (operation_id, from_path, to_path, size, state) \
                 VALUES (?1,?2,?3,?4,?5)",
            )?;
            for item in items {
                stmt.execute(rusqlite::params![
                    operation_id,
                    path_to_blob(&item.from),
                    item.to.as_deref().map(path_to_blob),
                    item.size as i64,
                    ItemState::Pending.key(),
                ])?;
                item_ids.push(tx.last_insert_rowid());
            }
        }
        tx.commit()?;
        Ok(Begun {
            operation_id,
            item_ids,
        })
    })
}

pub fn set_state(db: &Db, operation_id: i64, state: OpState) -> rusqlite::Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE operations SET state = ?1 WHERE id = ?2",
            rusqlite::params![state.key(), operation_id],
        )
        .map(|_| ())
    })
}

/// Record that one item succeeded, along with where it actually ended up.
pub fn item_done(db: &Db, item_id: i64, final_to: Option<&Path>) -> rusqlite::Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE operation_items SET state = ?1, to_path = COALESCE(?2, to_path), error = NULL \
             WHERE id = ?3",
            rusqlite::params![ItemState::Done.key(), final_to.map(path_to_blob), item_id],
        )
        .map(|_| ())
    })
}

pub fn item_failed(db: &Db, item_id: i64, error: &str) -> rusqlite::Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE operation_items SET state = ?1, error = ?2 WHERE id = ?3",
            rusqlite::params![ItemState::Failed.key(), error, item_id],
        )
        .map(|_| ())
    })
}

pub fn item_reverted(db: &Db, item_id: i64) -> rusqlite::Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE operation_items SET state = ?1 WHERE id = ?2",
            rusqlite::params![ItemState::Reverted.key(), item_id],
        )
        .map(|_| ())
    })
}

pub fn load(db: &Db, operation_id: i64) -> rusqlite::Result<Option<OperationRecord>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, kind, root, created_at, state, note FROM operations WHERE id = ?1",
        )?;
        let mut rows = stmt.query([operation_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let record = read_operation(row)?;
        drop(rows);
        drop(stmt);
        Ok(Some(with_items(conn, record)?))
    })
}

/// Most recent operations first, for the history list.
pub fn recent(db: &Db, limit: u32) -> rusqlite::Result<Vec<OperationRecord>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, kind, root, created_at, state, note FROM operations \
             ORDER BY created_at DESC, id DESC LIMIT ?1",
        )?;
        let mut records = Vec::new();
        let mut rows = stmt.query([limit.clamp(1, 200)])?;
        while let Some(row) = rows.next()? {
            records.push(read_operation(row)?);
        }
        drop(rows);
        drop(stmt);
        records
            .into_iter()
            .map(|record| with_items(conn, record))
            .collect()
    })
}

/// Operations left `planned` or `running` - meaning the app died mid-batch.
///
/// Surfaced on next launch so the user is told, rather than discovering a
/// half-organised folder on their own.
pub fn unfinished(db: &Db) -> rusqlite::Result<Vec<OperationRecord>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, kind, root, created_at, state, note FROM operations \
             WHERE state IN ('planned','running') ORDER BY created_at DESC",
        )?;
        let mut records = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            records.push(read_operation(row)?);
        }
        drop(rows);
        drop(stmt);
        records
            .into_iter()
            .map(|record| with_items(conn, record))
            .collect()
    })
}

/// Everything this app has ever tidied up, for the user, from the journal.
///
/// No extra bookkeeping: the journal already records every item that landed,
/// so the running total is a query rather than a second source of truth that
/// could drift from it. Items later undone stop counting, which is the honest
/// answer - the space came back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lifetime {
    pub files: u64,
    pub bytes: u64,
    pub operations: u64,
}

pub fn lifetime(db: &Db) -> rusqlite::Result<Lifetime> {
    db.with_conn(|conn| {
        let (files, bytes) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM operation_items WHERE state = 'done'",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let operations = conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE state IN ('done','partial')",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(Lifetime {
            files: files.max(0) as u64,
            bytes: bytes.max(0) as u64,
            operations: operations.max(0) as u64,
        })
    })
}

/// Settle a recovered operation: whatever was attempted, record the truth.
pub fn reconcile_unfinished(db: &Db) -> rusqlite::Result<usize> {
    let stale = unfinished(db)?;
    let count = stale.len();
    for operation in stale {
        // Pending items were never attempted, or were attempted and we cannot
        // know. Either way they are not "done"; mark the operation by what the
        // items actually say.
        let state = if operation.done_count() == 0 {
            OpState::Failed
        } else {
            OpState::Partial
        };
        set_state(db, operation.id, state)?;
    }
    Ok(count)
}

fn read_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    Ok(OperationRecord {
        id: row.get(0)?,
        kind: OpKind::parse(&row.get::<_, String>(1)?),
        root: blob_to_path(&row.get::<_, Vec<u8>>(2)?),
        created_at: row.get(3)?,
        state: OpState::parse(&row.get::<_, String>(4)?),
        note: row.get(5)?,
        items: Vec::new(),
    })
}

fn with_items(
    conn: &rusqlite::Connection,
    mut record: OperationRecord,
) -> rusqlite::Result<OperationRecord> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, from_path, to_path, size, state, error FROM operation_items \
         WHERE operation_id = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([record.id])?;
    while let Some(row) = rows.next()? {
        record.items.push(ItemRecord {
            id: row.get(0)?,
            from: blob_to_path(&row.get::<_, Vec<u8>>(1)?),
            to: row.get::<_, Option<Vec<u8>>>(2)?.map(|b| blob_to_path(&b)),
            size: row.get::<_, i64>(3)?.max(0) as u64,
            state: ItemState::parse(&row.get::<_, String>(4)?),
            error: row.get(5)?,
        });
    }
    Ok(record)
}

// --------------------------------------------------------- path encoding ---

#[cfg(windows)]
fn path_to_blob(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

#[cfg(windows)]
fn blob_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    PathBuf::from(std::ffi::OsString::from_wide(&wide))
}

#[cfg(unix)]
fn path_to_blob(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn blob_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn items() -> Vec<PlannedItem> {
        vec![
            PlannedItem {
                from: PathBuf::from("/root/a.png"),
                to: Some(PathBuf::from("/root/Images/a.png")),
                size: 100,
            },
            PlannedItem {
                from: PathBuf::from("/root/b.pdf"),
                to: Some(PathBuf::from("/root/Documents/b.pdf")),
                size: 200,
            },
        ]
    }

    #[test]
    fn a_plan_is_durable_before_anything_is_attempted() {
        let db = db();
        let begun = begin(&db, OpKind::Organize, Path::new("/root"), "tidy", &items()).unwrap();

        let record = load(&db, begun.operation_id).unwrap().unwrap();
        assert_eq!(record.state, OpState::Planned);
        assert_eq!(record.items.len(), 2);
        assert!(record.items.iter().all(|i| i.state == ItemState::Pending));
        assert_eq!(record.note, "tidy");
    }

    #[test]
    fn item_outcomes_are_recorded_individually() {
        let db = db();
        let begun = begin(&db, OpKind::Organize, Path::new("/root"), "", &items()).unwrap();
        set_state(&db, begun.operation_id, OpState::Running).unwrap();

        item_done(&db, begun.item_ids[0], Some(Path::new("/root/Images/a (2).png"))).unwrap();
        item_failed(&db, begun.item_ids[1], "in use by another program").unwrap();
        set_state(&db, begun.operation_id, OpState::Partial).unwrap();

        let record = load(&db, begun.operation_id).unwrap().unwrap();
        assert_eq!(record.state, OpState::Partial);
        assert_eq!(record.done_count(), 1);
        assert_eq!(record.failed_count(), 1);
        assert_eq!(record.bytes(), 100);

        // The rename that actually happened is what gets stored, not the plan.
        assert_eq!(
            record.items[0].to.as_deref(),
            Some(Path::new("/root/Images/a (2).png"))
        );
        assert_eq!(
            record.items[1].error.as_deref(),
            Some("in use by another program")
        );
    }

    #[test]
    fn a_partly_applied_operation_is_still_reversible() {
        assert!(OpState::Partial.is_reversible());
        assert!(OpState::Done.is_reversible());
        assert!(!OpState::Planned.is_reversible());
        assert!(!OpState::Failed.is_reversible());
        assert!(!OpState::Undone.is_reversible());
    }

    #[test]
    fn a_crash_leaves_an_unfinished_operation_that_recovery_settles() {
        let db = db();
        let crashed = begin(&db, OpKind::Organize, Path::new("/root"), "", &items()).unwrap();
        set_state(&db, crashed.operation_id, OpState::Running).unwrap();
        item_done(&db, crashed.item_ids[0], None).unwrap();
        // ... and here the process dies, leaving state = running.

        let stale = unfinished(&db).unwrap();
        assert_eq!(stale.len(), 1, "the crash must be visible on next launch");

        assert_eq!(reconcile_unfinished(&db).unwrap(), 1);
        let settled = load(&db, crashed.operation_id).unwrap().unwrap();
        assert_eq!(settled.state, OpState::Partial, "one item did land");
        assert!(unfinished(&db).unwrap().is_empty(), "settled, not re-reported");
    }

    #[test]
    fn a_crash_before_anything_happened_settles_as_failed_not_partial() {
        let db = db();
        let crashed = begin(&db, OpKind::Trash, Path::new("/root"), "", &items()).unwrap();
        set_state(&db, crashed.operation_id, OpState::Running).unwrap();

        reconcile_unfinished(&db).unwrap();
        let settled = load(&db, crashed.operation_id).unwrap().unwrap();
        assert_eq!(settled.state, OpState::Failed);
    }

    #[test]
    fn history_is_newest_first_and_bounded() {
        let db = db();
        for _ in 0..5 {
            begin(&db, OpKind::Trash, Path::new("/root"), "", &items()).unwrap();
        }
        let recent = recent(&db, 3).unwrap();
        assert_eq!(recent.len(), 3);
        assert!(recent[0].id > recent[2].id, "newest first");
        assert_eq!(recent[0].items.len(), 2, "items come along");
    }

    #[test]
    fn undo_marks_items_reverted_and_the_operation_undone() {
        let db = db();
        let begun = begin(&db, OpKind::Organize, Path::new("/root"), "", &items()).unwrap();
        item_done(&db, begun.item_ids[0], None).unwrap();
        item_done(&db, begun.item_ids[1], None).unwrap();
        set_state(&db, begun.operation_id, OpState::Done).unwrap();

        for id in &begun.item_ids {
            item_reverted(&db, *id).unwrap();
        }
        set_state(&db, begun.operation_id, OpState::Undone).unwrap();

        let record = load(&db, begun.operation_id).unwrap().unwrap();
        assert_eq!(record.state, OpState::Undone);
        assert!(record.items.iter().all(|i| i.state == ItemState::Reverted));
        assert!(!record.state.is_reversible(), "undo is not itself undoable");
    }

    #[test]
    fn paths_survive_a_round_trip_byte_for_byte() {
        // Including the kind of name that a lossy TEXT column would mangle.
        let db = db();
        let awkward = PathBuf::from("/root/re\u{0301}sume\u{0301} \u{2014} final (1).pdf");
        let begun = begin(
            &db,
            OpKind::Trash,
            Path::new("/root"),
            "",
            &[PlannedItem {
                from: awkward.clone(),
                to: None,
                size: 1,
            }],
        )
        .unwrap();

        let record = load(&db, begun.operation_id).unwrap().unwrap();
        assert_eq!(
            record.items[0].from, awkward,
            "an exact path is the difference between undo working and not"
        );
    }

    #[test]
    fn lifetime_counts_what_landed_and_forgets_what_was_undone() {
        let db = db();
        assert_eq!(lifetime(&db).unwrap(), Lifetime::default());

        let first = begin(&db, OpKind::Trash, Path::new("/root"), "", &items()).unwrap();
        item_done(&db, first.item_ids[0], None).unwrap();
        item_done(&db, first.item_ids[1], None).unwrap();
        set_state(&db, first.operation_id, OpState::Done).unwrap();

        let total = lifetime(&db).unwrap();
        assert_eq!(total.files, 2);
        assert_eq!(total.bytes, 300);
        assert_eq!(total.operations, 1);

        // Undoing gives the space back, so the running total must give it back
        // too rather than overstating what was achieved.
        for id in &first.item_ids {
            item_reverted(&db, *id).unwrap();
        }
        set_state(&db, first.operation_id, OpState::Undone).unwrap();
        assert_eq!(lifetime(&db).unwrap(), Lifetime::default());
    }

    #[test]
    fn a_failed_item_never_counts_towards_the_total() {
        let db = db();
        let begun = begin(&db, OpKind::Trash, Path::new("/root"), "", &items()).unwrap();
        item_done(&db, begun.item_ids[0], None).unwrap();
        item_failed(&db, begun.item_ids[1], "in use").unwrap();
        set_state(&db, begun.operation_id, OpState::Partial).unwrap();

        let total = lifetime(&db).unwrap();
        assert_eq!(total.files, 1);
        assert_eq!(total.bytes, 100);
    }

    #[test]
    fn loading_a_missing_operation_is_none_not_an_error() {
        let db = db();
        assert!(load(&db, 9999).unwrap().is_none());
    }
}
