//! The bookkeeping a vault's local file keeps for sync.
//!
//! Every vault lives in a local SQLite file, synced or not, and the app only
//! ever reads and writes that file. Triggers on `secrets` note each change in
//! `sync_dirty` (which record, when, and whether it was deleted), so nothing
//! that writes a secret has to know about sync. The sync engine sends those
//! changes to the vault's backend and applies what other devices sent, with
//! the triggers held off (a row in `sync_applying`) so that what came from the
//! backend is not sent straight back.
//!
//! `sync_known` holds the version of each record last exchanged with the
//! backend, so an old copy that a backend serves again is ignored, and
//! `sync_state` holds the vault's cursor, format and last result.

use libsql::Connection;

use crate::vault::error::VaultError;

pub(super) const KEY_EPOCH_STATE: &str = "organization_key_epoch";
pub(super) const ROTATION_PENDING_STATE: &str = "organization_rotation_pending";

/// One secret as the `secrets` table stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub category: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub wrapped_dek: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A change made on this device and not yet sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirty {
    pub id: String,
    /// When it was made, in milliseconds since the epoch.
    pub changed_at: i64,
    pub deleted: bool,
}

/// Milliseconds since the epoch, as the triggers write it.
const NOW_MS: &str = "CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)";

/// Create the sync tables and triggers. Safe to run on every open.
pub async fn init(conn: &Connection) -> Result<(), VaultError> {
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS sync_dirty (
            id TEXT PRIMARY KEY,
            changed_at INTEGER NOT NULL,
            deleted INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sync_known (
            id TEXT PRIMARY KEY,
            version INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sync_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sync_applying (x INTEGER);

        CREATE TRIGGER IF NOT EXISTS sync_secret_inserted AFTER INSERT ON secrets
        WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
        BEGIN
            INSERT OR REPLACE INTO sync_dirty (id, changed_at, deleted) VALUES (NEW.id, {NOW_MS}, 0);
        END;
        CREATE TRIGGER IF NOT EXISTS sync_secret_updated AFTER UPDATE ON secrets
        WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
        BEGIN
            INSERT OR REPLACE INTO sync_dirty (id, changed_at, deleted) VALUES (NEW.id, {NOW_MS}, 0);
            INSERT OR REPLACE INTO sync_dirty (id, changed_at, deleted)
                SELECT OLD.id, {NOW_MS}, 1 WHERE OLD.id <> NEW.id;
        END;
        CREATE TRIGGER IF NOT EXISTS sync_secret_deleted AFTER DELETE ON secrets
        WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
        BEGIN
            INSERT OR REPLACE INTO sync_dirty (id, changed_at, deleted) VALUES (OLD.id, {NOW_MS}, 1);
        END;
        "#
    ))
    .await?;
    Ok(())
}

const ROW_COLUMNS: &str = "id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at";

fn row_from(row: &libsql::Row) -> Result<Row, VaultError> {
    Ok(Row {
        id: row.get(0)?,
        name: row.get(1)?,
        category: row.get(2)?,
        nonce: row.get(3)?,
        ciphertext: row.get(4)?,
        wrapped_dek: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// One secret's row, if it is there.
pub async fn read_row(conn: &Connection, id: &str) -> Result<Option<Row>, VaultError> {
    let mut rows = conn
        .query(&format!("SELECT {ROW_COLUMNS} FROM secrets WHERE id = ?"), [id])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row_from(&row)?)),
        None => Ok(None),
    }
}

/// Every secret's row.
pub async fn all_rows(conn: &Connection) -> Result<Vec<Row>, VaultError> {
    let mut rows = conn.query(&format!("SELECT {ROW_COLUMNS} FROM secrets ORDER BY id"), ()).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(row_from(&row)?);
    }
    Ok(out)
}

/// Every change not yet sent, oldest first.
pub async fn dirty(conn: &Connection) -> Result<Vec<Dirty>, VaultError> {
    let mut rows = conn
        .query("SELECT id, changed_at, deleted FROM sync_dirty ORDER BY changed_at, id", ())
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Dirty { id: row.get(0)?, changed_at: row.get(1)?, deleted: row.get::<i64>(2)? != 0 });
    }
    Ok(out)
}

/// How many changes wait to be sent.
pub async fn dirty_count(conn: &Connection) -> Result<usize, VaultError> {
    let mut rows = conn.query("SELECT COUNT(*) FROM sync_dirty", ()).await?;
    Ok(match rows.next().await? {
        Some(row) => row.get::<i64>(0)? as usize,
        None => 0,
    })
}

/// Forget a sent change, unless it changed again since `changed_at`.
pub async fn clear_dirty(conn: &Connection, id: &str, changed_at: i64) -> Result<(), VaultError> {
    conn.execute("DELETE FROM sync_dirty WHERE id = ? AND changed_at <= ?", (id, changed_at))
        .await?;
    Ok(())
}

/// The version of a record last exchanged with the backend.
pub async fn known(conn: &Connection, id: &str) -> Result<Option<i64>, VaultError> {
    let mut rows = conn.query("SELECT version FROM sync_known WHERE id = ?", [id]).await?;
    Ok(match rows.next().await? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub async fn set_known(conn: &Connection, id: &str, version: i64) -> Result<(), VaultError> {
    conn.execute("INSERT OR REPLACE INTO sync_known (id, version) VALUES (?, ?)", (id, version))
        .await?;
    Ok(())
}

pub async fn state(conn: &Connection, key: &str) -> Result<Option<String>, VaultError> {
    let mut rows = conn.query("SELECT value FROM sync_state WHERE key = ?", [key]).await?;
    Ok(match rows.next().await? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub async fn set_state(conn: &Connection, key: &str, value: &str) -> Result<(), VaultError> {
    conn.execute("INSERT OR REPLACE INTO sync_state (key, value) VALUES (?, ?)", (key, value))
        .await?;
    Ok(())
}

pub async fn clear_state(conn: &Connection, key: &str) -> Result<(), VaultError> {
    conn.execute("DELETE FROM sync_state WHERE key = ?", [key]).await?;
    Ok(())
}

/// Put what the backend sent into the file: a row, or its deletion. The
/// triggers are held off, so this is not counted as a change of this device.
///
/// It runs in a write transaction: the marker that holds the triggers off is
/// never seen by another connection, so a change the app makes meanwhile
/// (which waits for the transaction) is still noted.
pub async fn apply(conn: &Connection, id: &str, row: Option<&Row>) -> Result<(), VaultError> {
    let tx = conn.transaction_with_behavior(libsql::TransactionBehavior::Immediate).await?;
    apply_in_transaction(&tx, id, row).await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn apply_in_transaction(conn: &Connection, id: &str, row: Option<&Row>) -> Result<(), VaultError> {
    conn.execute("INSERT INTO sync_applying (x) VALUES (1)", ()).await?;
    match row {
        Some(r) => conn.execute(
            &format!("INSERT OR REPLACE INTO secrets ({ROW_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"),
            (
                r.id.as_str(), r.name.as_str(), r.category.as_str(), r.nonce.as_slice(),
                r.ciphertext.as_slice(), r.wrapped_dek.as_str(), r.created_at, r.updated_at,
            ),
        ).await?,
        None => conn.execute("DELETE FROM secrets WHERE id = ?", [id]).await?,
    };
    conn.execute("DELETE FROM sync_applying", ()).await?;
    Ok(())
}

/// Start over: forget what was exchanged with a backend, so the next sync
/// reads it in full. Pending changes and the active organization encryption
/// state belong to the local vault, so they survive a backend reset.
pub async fn reset(conn: &Connection) -> Result<(), VaultError> {
    conn.execute_batch(&format!(
        "DELETE FROM sync_known; DELETE FROM sync_applying;
         DELETE FROM sync_state WHERE key NOT IN ('{KEY_EPOCH_STATE}', '{ROTATION_PENDING_STATE}');"
    )).await?;
    Ok(())
}

/// Count every row of the vault as a change of this device, as when a vault
/// that only lived here starts syncing.
pub async fn mark_all_dirty(conn: &Connection) -> Result<(), VaultError> {
    conn.execute(
        &format!("INSERT OR REPLACE INTO sync_dirty (id, changed_at, deleted) SELECT id, {NOW_MS}, 0 FROM secrets"),
        (),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) async fn memory_vault() -> Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        crate::vault::schema::init_schema(&conn).await.unwrap();
        init(&conn).await.unwrap();
        conn
    }

    fn row(id: &str, name: &str) -> Row {
        Row {
            id: id.into(),
            name: name.into(),
            category: "session".into(),
            nonce: vec![1; 24],
            ciphertext: vec![2, 3],
            wrapped_dek: "{}".into(),
            created_at: 1,
            updated_at: 2,
        }
    }

    async fn write(conn: &Connection, r: &Row) {
        conn.execute(
            &format!("INSERT OR REPLACE INTO secrets ({ROW_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"),
            (r.id.as_str(), r.name.as_str(), r.category.as_str(), r.nonce.as_slice(), r.ciphertext.as_slice(), r.wrapped_dek.as_str(), r.created_at, r.updated_at),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn organization_key_state_survives_a_sync_reset() {
        let conn = memory_vault().await;
        set_state(&conn, KEY_EPOCH_STATE, "3").await.unwrap();
        set_state(&conn, ROTATION_PENDING_STATE, "1").await.unwrap();
        set_state(&conn, "previous_revision", "tag").await.unwrap();
        reset(&conn).await.unwrap();
        assert_eq!(state(&conn, KEY_EPOCH_STATE).await.unwrap().as_deref(), Some("3"));
        assert_eq!(state(&conn, ROTATION_PENDING_STATE).await.unwrap().as_deref(), Some("1"));
        assert!(state(&conn, "previous_revision").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn writes_here_are_noted_and_applied_ones_are_not() {
        let conn = memory_vault().await;
        write(&conn, &row("a", "one")).await;
        conn.execute("UPDATE secrets SET name = 'uno' WHERE id = 'a'", ()).await.unwrap();
        apply(&conn, "b", Some(&row("b", "from elsewhere"))).await.unwrap();
        let d = dirty(&conn).await.unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "a");
        assert!(!d[0].deleted);
        assert!(d[0].changed_at > 1_600_000_000_000, "milliseconds since the epoch");

        conn.execute("DELETE FROM secrets WHERE id = 'a'", ()).await.unwrap();
        apply(&conn, "b", None).await.unwrap();
        let d = dirty(&conn).await.unwrap();
        assert_eq!(d.len(), 1);
        assert!(d[0].deleted);
        assert!(read_row(&conn, "b").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_change_made_while_sending_is_kept() {
        let conn = memory_vault().await;
        write(&conn, &row("a", "one")).await;
        let sent = dirty(&conn).await.unwrap().remove(0);
        conn.execute("UPDATE sync_dirty SET changed_at = changed_at + 10", ()).await.unwrap();
        clear_dirty(&conn, "a", sent.changed_at).await.unwrap();
        assert_eq!(dirty_count(&conn).await.unwrap(), 1);
    }
}
