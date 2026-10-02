use crate::algorithm;
use crate::feedback::{self, FeedbackResolution};
use crate::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn migration_error(message: &str) -> Error {
    Error("DB_ERROR", message.into())
}

fn verify_index_definition(
    conn: &Connection,
    name: &str,
    table: &str,
    expected_columns: &[(&str, bool)],
) -> Result<()> {
    let index_table: Option<String> = conn
        .query_row(
            "SELECT tbl_name FROM sqlite_master WHERE type='index' AND name=?",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    if index_table.as_deref() != Some(table) {
        return Err(migration_error(
            "A required database index has an unexpected definition.",
        ));
    }

    let flags: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT \"unique\",origin,partial FROM pragma_index_list(?1) WHERE name=?2",
            params![table, name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if flags
        .as_ref()
        .is_none_or(|(unique, origin, partial)| *unique != 0 || origin != "c" || *partial != 0)
    {
        return Err(migration_error(
            "A required database index has an unexpected definition.",
        ));
    }

    // The index names are fixed constants at the call sites, so the PRAGMA
    // identifier is not derived from database contents.
    let sql = format!("PRAGMA index_xinfo(\"{name}\")");
    let actual = conn
        .prepare(&sql)?
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .filter_map(|row| match row {
            Ok((column, descending, collation, is_key)) if is_key != 0 => {
                Some(Ok((column, descending, collation)))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let expected: Vec<_> = expected_columns
        .iter()
        .map(|(column, descending)| {
            (
                Some((*column).to_string()),
                *descending,
                Some("BINARY".to_string()),
            )
        })
        .collect();
    if actual != expected {
        return Err(migration_error(
            "A required database index has an unexpected definition.",
        ));
    }
    Ok(())
}

fn create_and_verify_targeted_indexes(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS atoms_by_source_recency
             ON atoms(source, last_seen DESC, id);
         CREATE INDEX IF NOT EXISTS memberships_by_topic
             ON atom_topics(topic, atom);
         CREATE INDEX IF NOT EXISTS feedback_by_atom_action
             ON feedback(atom, action, seq);",
    )?;
    verify_index_definition(
        tx,
        "atoms_by_source_recency",
        "atoms",
        &[("source", false), ("last_seen", true), ("id", false)],
    )?;
    verify_index_definition(
        tx,
        "memberships_by_topic",
        "atom_topics",
        &[("topic", false), ("atom", false)],
    )?;
    verify_index_definition(
        tx,
        "feedback_by_atom_action",
        "feedback",
        &[("atom", false), ("action", false), ("seq", false)],
    )?;
    Ok(())
}

fn verify_foreign_keys(tx: &Transaction<'_>) -> Result<()> {
    let has_violation = {
        let mut statement = tx.prepare("PRAGMA foreign_key_check")?;
        let violation = statement.query([])?.next()?.is_some();
        violation
    };
    if has_violation {
        return Err(migration_error(
            "The feedback schema migration found a foreign-key violation.",
        ));
    }
    Ok(())
}

fn verify_feedback_copy(tx: &Transaction<'_>) -> Result<()> {
    let old_count: i64 = tx.query_row("SELECT count(*) FROM feedback", [], |row| row.get(0))?;
    let new_count: i64 = tx.query_row("SELECT count(*) FROM feedback_v4", [], |row| row.get(0))?;
    if old_count != new_count {
        return Err(migration_error(
            "The feedback schema migration did not preserve every row.",
        ));
    }
    let mismatched_rows: i64 = tx.query_row(
        "SELECT count(*) FROM feedback AS old
         LEFT JOIN feedback_v4 AS new ON new.id=old.id
         WHERE new.id IS NULL
            OR old.atom IS NOT new.atom
            OR old.action IS NOT new.action
            OR old.text IS NOT new.text
            OR old.time IS NOT new.time",
        [],
        |row| row.get(0),
    )?;
    let wrong_sequence: i64 = tx.query_row(
        "SELECT count(*) FROM (
             SELECT new.seq AS seq,
                    row_number() OVER (ORDER BY old.rowid ASC) AS expected_seq
             FROM feedback AS old
             JOIN feedback_v4 AS new ON new.id=old.id
         ) WHERE seq != expected_seq",
        [],
        |row| row.get(0),
    )?;
    if mismatched_rows != 0 || wrong_sequence != 0 {
        return Err(migration_error(
            "The feedback schema migration changed row data or sequence order.",
        ));
    }
    Ok(())
}

const TOPIC_CACHE_TRIGGER_DDLS: &[(&str, &str, &str)] = &[
    (
        "topic_cache_atom_days_insert",
        "atom_days",
        "CREATE TRIGGER topic_cache_atom_days_insert AFTER INSERT ON atom_days BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=NEW.atom);
         END;",
    ),
    (
        "topic_cache_atom_days_update",
        "atom_days",
        "CREATE TRIGGER topic_cache_atom_days_update AFTER UPDATE ON atom_days BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (
                 SELECT topic FROM atom_topics WHERE atom=OLD.atom
                 UNION SELECT topic FROM atom_topics WHERE atom=NEW.atom
             );
         END;",
    ),
    (
        "topic_cache_atom_days_delete",
        "atom_days",
        "CREATE TRIGGER topic_cache_atom_days_delete AFTER DELETE ON atom_days BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=OLD.atom);
         END;",
    ),
    (
        "topic_cache_atom_topics_insert",
        "atom_topics",
        "CREATE TRIGGER topic_cache_atom_topics_insert AFTER INSERT ON atom_topics BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1 WHERE topic=NEW.topic;
         END;",
    ),
    (
        "topic_cache_atom_topics_update",
        "atom_topics",
        "CREATE TRIGGER topic_cache_atom_topics_update AFTER UPDATE ON atom_topics BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (OLD.topic,NEW.topic);
         END;",
    ),
    (
        "topic_cache_atom_topics_delete",
        "atom_topics",
        "CREATE TRIGGER topic_cache_atom_topics_delete AFTER DELETE ON atom_topics BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1 WHERE topic=OLD.topic;
         END;",
    ),
    (
        "topic_cache_vectors_insert",
        "vectors",
        "CREATE TRIGGER topic_cache_vectors_insert AFTER INSERT ON vectors BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=NEW.atom);
         END;",
    ),
    (
        "topic_cache_vectors_update",
        "vectors",
        "CREATE TRIGGER topic_cache_vectors_update AFTER UPDATE ON vectors BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (
                 SELECT topic FROM atom_topics WHERE atom=OLD.atom
                 UNION SELECT topic FROM atom_topics WHERE atom=NEW.atom
             );
         END;",
    ),
    (
        "topic_cache_vectors_delete",
        "vectors",
        "CREATE TRIGGER topic_cache_vectors_delete AFTER DELETE ON vectors BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=OLD.atom);
         END;",
    ),
    (
        "topic_cache_feedback_insert",
        "feedback",
        "CREATE TRIGGER topic_cache_feedback_insert AFTER INSERT ON feedback BEGIN
             UPDATE topic_cache SET dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=NEW.atom);
         END;",
    ),
    (
        "topic_cache_feedback_update",
        "feedback",
        "CREATE TRIGGER topic_cache_feedback_update AFTER UPDATE ON feedback BEGIN
             UPDATE topic_cache SET dirty_label=1
             WHERE topic IN (
                 SELECT topic FROM atom_topics WHERE atom=OLD.atom
                 UNION SELECT topic FROM atom_topics WHERE atom=NEW.atom
             );
         END;",
    ),
    (
        "topic_cache_feedback_delete",
        "feedback",
        "CREATE TRIGGER topic_cache_feedback_delete AFTER DELETE ON feedback BEGIN
             UPDATE topic_cache SET dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=OLD.atom);
         END;",
    ),
    (
        "topic_cache_bucket_insert",
        "topic_cache_buckets",
        "CREATE TRIGGER topic_cache_bucket_insert AFTER INSERT ON topic_cache_buckets BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1 WHERE topic=NEW.topic;
         END;",
    ),
    (
        "topic_cache_bucket_update",
        "topic_cache_buckets",
        "CREATE TRIGGER topic_cache_bucket_update AFTER UPDATE ON topic_cache_buckets BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (OLD.topic,NEW.topic);
         END;",
    ),
    (
        "topic_cache_bucket_delete",
        "topic_cache_buckets",
        "CREATE TRIGGER topic_cache_bucket_delete AFTER DELETE ON topic_cache_buckets BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1 WHERE topic=OLD.topic;
         END;",
    ),
    (
        "topic_cache_atoms_update",
        "atoms",
        "CREATE TRIGGER topic_cache_atoms_update
         AFTER UPDATE OF site,source,title,query ON atoms BEGIN
             UPDATE topic_cache SET valid=0,dirty_label=1
             WHERE topic IN (SELECT topic FROM atom_topics WHERE atom=NEW.id);
         END;",
    ),
];

fn normalize_schema_sql(sql: &str) -> String {
    let compact = sql
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    compact.trim_end_matches(';').to_owned()
}

fn verify_topic_cache_schema(conn: &Connection) -> Result<()> {
    for (table, expected_columns) in [
        (
            "topic_cache",
            vec![
                ("topic", "TEXT", 1, 1),
                ("model", "TEXT", 1, 0),
                ("valid", "INTEGER", 1, 0),
                ("sum", "BLOB", 1, 0),
                ("sum_hash", "BLOB", 1, 0),
                ("dirty_label", "INTEGER", 1, 0),
            ],
        ),
        (
            "topic_cache_buckets",
            vec![
                ("topic", "TEXT", 1, 1),
                ("site", "TEXT", 1, 2),
                ("session", "TEXT", 1, 3),
                ("mass", "REAL", 1, 0),
                ("resultant", "BLOB", 1, 0),
                ("resultant_hash", "BLOB", 1, 0),
            ],
        ),
    ] {
        let exists: i64 = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if exists != 1 {
            return Err(migration_error("The topic cache schema is incomplete."));
        }
        let sql = format!("PRAGMA table_info({table})");
        let columns = conn
            .prepare(&sql)?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let expected: Vec<_> = expected_columns
            .iter()
            .map(|(name, kind, not_null, key_order)| {
                (
                    (*name).to_owned(),
                    (*kind).to_owned(),
                    *not_null,
                    *key_order,
                )
            })
            .collect();
        if columns != expected {
            return Err(migration_error(
                "The topic cache table definition is incompatible.",
            ));
        }
    }
    for (table, referenced_table, from, to) in [
        ("topic_cache", "topics", "topic", "id"),
        ("topic_cache_buckets", "topic_cache", "topic", "topic"),
    ] {
        let sql = format!("PRAGMA foreign_key_list({table})");
        let foreign_keys = conn
            .prepare(&sql)?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if foreign_keys
            != vec![(
                referenced_table.to_owned(),
                from.to_owned(),
                to.to_owned(),
                "CASCADE".to_owned(),
            )]
        {
            return Err(migration_error(
                "The topic cache foreign-key definition is incompatible.",
            ));
        }
    }
    for (table, required) in [
        (
            "topic_cache",
            [
                "check(typeof(sum)='blob'andlength(sum)=2048)",
                "check(typeof(sum_hash)='blob'andlength(sum_hash)=32)",
                "check(validin(0,1))",
                "check(dirty_labelin(0,1))",
            ],
        ),
        (
            "topic_cache_buckets",
            [
                "check(typeof(mass)in('real','integer')andmass>=0)",
                "check(typeof(resultant)='blob'andlength(resultant)=2048)",
                "check(typeof(resultant_hash)='blob'andlength(resultant_hash)=32)",
                "primarykey(topic,site,session)",
            ],
        ),
    ] {
        let sql: String = conn.query_row(
            "SELECT lower(sql) FROM sqlite_master WHERE type='table' AND name=?",
            [table],
            |row| row.get(0),
        )?;
        let compact = sql
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        if required.iter().any(|clause| !compact.contains(clause)) {
            return Err(migration_error(
                "The topic cache storage constraints are incompatible.",
            ));
        }
    }
    for (name, expected_table, expected_sql) in TOPIC_CACHE_TRIGGER_DDLS {
        let definition: Option<(String, String)> = conn
            .query_row(
                "SELECT tbl_name,sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                [name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if definition.as_ref().is_none_or(|(table, sql)| {
            table != expected_table
                || normalize_schema_sql(sql) != normalize_schema_sql(expected_sql)
        }) {
            return Err(migration_error(
                "The topic cache invalidation schema is incompatible.",
            ));
        }
    }
    Ok(())
}

/// Keep derived cache growth inside the existing database-plus-WAL storage gate.
/// Leave a small reserve for the current authoritative row changes in a refresh
/// transaction; the cache estimator separately accounts for each SQLite page
/// and WAL frame it may add.
fn topic_cache_headroom_bytes(conn: &Connection, path: &Path) -> Result<u64> {
    const STORAGE_LIMIT: u64 = 96 * 1024 * 1024;
    const REFRESH_RESERVE: u64 = 64 * 4096;
    let database_bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut wal_path = path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_bytes = std::fs::metadata(PathBuf::from(wal_path))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    // Reading the page geometry here keeps the estimate coupled to the actual
    // SQLite layout, while the admission gate itself continues to use file
    // sizes exactly as it does for ordinary ingestion.
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    // Cache estimates assume SQLite's default 4 KiB pages. Smaller legal page
    // sizes can overflow those estimates when a 2 KiB vector accumulator spans
    // many B-tree pages, so leave the authoritative path uncached there.
    if page_size < 4096 {
        return Ok(0);
    }
    let page_reserve = (page_size.max(4096) as u64).saturating_mul(64);
    Ok(STORAGE_LIMIT
        .saturating_sub(database_bytes.saturating_add(wal_bytes))
        .saturating_sub(REFRESH_RESERVE.max(page_reserve)))
}

fn migrate_topic_cache_4_to_5(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 4 {
        return Err(migration_error(
            "The topic cache migration requires schema version 4.",
        ));
    }
    tx.execute_batch(
        "CREATE TABLE topic_cache (
             topic TEXT NOT NULL PRIMARY KEY REFERENCES topics(id) ON DELETE CASCADE,
             model TEXT NOT NULL,
             valid INTEGER NOT NULL CHECK(valid IN (0,1)),
             sum BLOB NOT NULL CHECK(typeof(sum)='blob' AND length(sum)=2048),
             sum_hash BLOB NOT NULL CHECK(typeof(sum_hash)='blob' AND length(sum_hash)=32),
             dirty_label INTEGER NOT NULL CHECK(dirty_label IN (0,1))
         );
         CREATE TABLE topic_cache_buckets (
             topic TEXT NOT NULL REFERENCES topic_cache(topic) ON DELETE CASCADE,
             site TEXT NOT NULL,
             session TEXT NOT NULL,
             mass REAL NOT NULL CHECK(typeof(mass) IN ('real','integer') AND mass>=0),
             resultant BLOB NOT NULL CHECK(typeof(resultant)='blob' AND length(resultant)=2048),
             resultant_hash BLOB NOT NULL CHECK(typeof(resultant_hash)='blob' AND length(resultant_hash)=32),
             PRIMARY KEY(topic,site,session)
         );",
    )?;
    for (_, _, trigger_ddl) in TOPIC_CACHE_TRIGGER_DDLS {
        tx.execute_batch(trigger_ddl)?;
    }

    verify_topic_cache_schema(&tx)?;
    let cache_rows: i64 = tx.query_row(
        "SELECT (SELECT count(*) FROM topic_cache)
              + (SELECT count(*) FROM topic_cache_buckets)",
        [],
        |row| row.get(0),
    )?;
    if cache_rows != 0 {
        return Err(migration_error(
            "The schema migration unexpectedly populated derived topic caches.",
        ));
    }
    verify_foreign_keys(&tx)?;
    tx.execute_batch("PRAGMA user_version=5;")?;
    verify_foreign_keys(&tx)?;
    tx.commit()?;
    Ok(())
}

fn migrate_feedback_3_to_4(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 3 {
        return Err(migration_error(
            "The feedback schema migration requires schema version 3.",
        ));
    }
    tx.execute_batch(
        "CREATE TABLE feedback_v4 (
             seq INTEGER PRIMARY KEY AUTOINCREMENT,
             id TEXT NOT NULL UNIQUE,
             atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
             action TEXT NOT NULL,
             text TEXT,
             time TEXT NOT NULL
         );
         INSERT INTO feedback_v4(id,atom,action,text,time)
             SELECT id,atom,action,text,time FROM feedback ORDER BY rowid ASC;",
    )?;
    verify_feedback_copy(&tx)?;
    verify_foreign_keys(&tx)?;
    tx.execute_batch(
        "DROP TABLE feedback;
         ALTER TABLE feedback_v4 RENAME TO feedback;",
    )?;
    verify_foreign_keys(&tx)?;
    create_and_verify_targeted_indexes(&tx)?;
    tx.execute_batch("PRAGMA user_version=4;")?;
    tx.commit()?;
    Ok(())
}

pub struct Vault {
    pub conn: Connection,
    pub path: PathBuf,
}

#[derive(Debug)]
struct PendingAtom {
    id: String,
    title: String,
    query: Option<String>,
    stored_vector: Option<Vec<f32>>,
    needs_vector: bool,
    needs_topic: bool,
}

fn decode_vector_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|component| f32::from_le_bytes(component.try_into().unwrap()))
        .collect()
}

fn vector_blob_is_valid(blob: &[u8]) -> bool {
    blob.len() == algorithm::DIMENSIONS * 4
        && blob
            .chunks_exact(4)
            .all(|component| f32::from_le_bytes(component.try_into().unwrap()).is_finite())
}

impl Vault {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            install::private_dir(p)?
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_millis(1500))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-4096; PRAGMA wal_autocheckpoint=256; PRAGMA secure_delete=ON;")?;
        let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver > 5 {
            return Err(Error(
                "SCHEMA_TOO_NEW",
                "Update Serein before opening this vault.".into(),
            ));
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT OR IGNORE INTO meta VALUES('evidence','0'),('privacy','0');
CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY,policy TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS site_rules(source TEXT NOT NULL,site TEXT NOT NULL,epoch INTEGER NOT NULL,excluded INTEGER NOT NULL,PRIMARY KEY(source,site));
CREATE TABLE IF NOT EXISTS atoms(id TEXT PRIMARY KEY,source TEXT NOT NULL,site TEXT NOT NULL,title TEXT NOT NULL,query TEXT,kind TEXT NOT NULL,first_seen TEXT NOT NULL,last_seen TEXT NOT NULL,seconds INTEGER NOT NULL,canonical TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS events(source TEXT NOT NULL,id TEXT NOT NULL,atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,visit TEXT NOT NULL,time TEXT NOT NULL,PRIMARY KEY(source,id));
CREATE TABLE IF NOT EXISTS atom_days(atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,day TEXT NOT NULL,session TEXT NOT NULL,mass REAL NOT NULL,PRIMARY KEY(atom,day,session));
CREATE VIRTUAL TABLE IF NOT EXISTS atom_fts USING fts5(id UNINDEXED,title,query,tokenize='unicode61');
CREATE TRIGGER IF NOT EXISTS atom_insert AFTER INSERT ON atoms BEGIN INSERT INTO atom_fts(id,title,query) VALUES(new.id,new.title,new.query); END;
CREATE TRIGGER IF NOT EXISTS atom_delete AFTER DELETE ON atoms BEGIN DELETE FROM atom_fts WHERE id=old.id; END;
CREATE TABLE IF NOT EXISTS vectors(atom TEXT PRIMARY KEY REFERENCES atoms(id) ON DELETE CASCADE,model TEXT NOT NULL,vector BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS feedback(id TEXT PRIMARY KEY,atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,action TEXT NOT NULL,text TEXT,time TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS receipts(id TEXT PRIMARY KEY,ids TEXT NOT NULL,evidence INTEGER NOT NULL,privacy INTEGER NOT NULL,bytes INTEGER NOT NULL,time TEXT NOT NULL,status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS topics(id TEXT PRIMARY KEY,label TEXT NOT NULL,centroid BLOB NOT NULL,model TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS atom_topics(atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,topic TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,mass REAL NOT NULL,PRIMARY KEY(atom,topic));
CREATE TABLE IF NOT EXISTS topic_skips(atom TEXT PRIMARY KEY REFERENCES atoms(id) ON DELETE CASCADE,model TEXT NOT NULL);
CREATE TRIGGER IF NOT EXISTS topic_deleted_reconsider_skips AFTER DELETE ON topics BEGIN DELETE FROM topic_skips WHERE model=old.model; END;
        ")?;
        if ver < 2 {
            conn.execute_batch("BEGIN IMMEDIATE;
DROP TRIGGER IF EXISTS topic_invalidate;
CREATE TRIGGER topic_invalidate BEFORE DELETE ON atoms BEGIN DELETE FROM topics WHERE id IN (SELECT topic FROM atom_topics WHERE atom=old.id); END;
PRAGMA user_version=3;
COMMIT;")?;
        } else if ver < 3 {
            conn.execute_batch("PRAGMA user_version=3;")?;
        }
        let current_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if current_version < 4 {
            migrate_feedback_3_to_4(&mut conn)?;
        } else {
            verify_index_definition(
                &conn,
                "atoms_by_source_recency",
                "atoms",
                &[("source", false), ("last_seen", true), ("id", false)],
            )?;
            verify_index_definition(
                &conn,
                "memberships_by_topic",
                "atom_topics",
                &[("topic", false), ("atom", false)],
            )?;
            verify_index_definition(
                &conn,
                "feedback_by_atom_action",
                "feedback",
                &[("atom", false), ("action", false), ("seq", false)],
            )?;
        }
        let migrated_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if migrated_version == 4 {
            migrate_topic_cache_4_to_5(&mut conn)?;
        } else if migrated_version == 5 {
            verify_topic_cache_schema(&conn)?;
        } else {
            return Err(migration_error(
                "The database did not reach a supported schema version.",
            ));
        }
        conn.execute(
            "INSERT OR IGNORE INTO meta(key,value) VALUES('canonical_salt',?)",
            [id()],
        )?;
        install::private_file(path)?;
        Ok(Self {
            conn,
            path: path.into(),
        })
    }
    pub fn generation(&self) -> Result<(i64, i64)> {
        Ok((
            self.conn
                .query_row("SELECT value FROM meta WHERE key='evidence'", [], |r| {
                    r.get::<_, String>(0)
                })?
                .parse()
                .unwrap_or(0),
            self.conn
                .query_row("SELECT value FROM meta WHERE key='privacy'", [], |r| {
                    r.get::<_, String>(0)
                })?
                .parse()
                .unwrap_or(0),
        ))
    }
    fn current_model_hash(&self) -> Option<String> {
        model::current_hash(&root().join("models/current"))
    }
    fn pending_atoms_for_model(&self, model_hash: Option<&str>) -> Result<i64> {
        match model_hash {
            Some(hash) => {
                let mut statement = self.conn.prepare(
                    "SELECT v.model,v.vector,
                            EXISTS (
                                SELECT 1 FROM atom_topics m
                                JOIN topics t ON t.id=m.topic
                                WHERE m.atom=a.id AND t.model=?1
                            ),
                            EXISTS (
                                SELECT 1 FROM topic_skips s
                                WHERE s.atom=a.id AND s.model=?1
                            )
                     FROM atoms a
                     LEFT JOIN vectors v ON v.atom=a.id",
                )?;
                let mut rows = statement.query([hash])?;
                let mut pending = 0i64;
                while let Some(row) = rows.next()? {
                    let vector_model_matches = matches!(
                        row.get_ref(0)?,
                        rusqlite::types::ValueRef::Text(model) if model == hash.as_bytes()
                    );
                    let vector_blob = match row.get_ref(1)? {
                        rusqlite::types::ValueRef::Blob(blob) => Some(blob),
                        _ => None,
                    };
                    let vector_valid =
                        vector_model_matches && vector_blob.is_some_and(vector_blob_is_valid);
                    let topic_ready = row.get::<_, bool>(2)? || row.get::<_, bool>(3)?;
                    if !vector_valid || !topic_ready {
                        pending += 1;
                    }
                }
                Ok(pending)
            }
            None => Ok(self
                .conn
                .query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))?),
        }
    }
    fn pending_atom_work(
        &self,
        model_hash: &str,
        limit: i64,
        deadline: Option<(Instant, u64)>,
    ) -> Result<Vec<PendingAtom>> {
        let mut statement = self.conn.prepare(
            "SELECT a.id,a.title,a.query,v.model,v.vector,
                    EXISTS (
                        SELECT 1 FROM atom_topics m
                        JOIN topics t ON t.id=m.topic
                        WHERE m.atom=a.id AND t.model=?1
                    ),
                    EXISTS (
                        SELECT 1 FROM topic_skips s
                        WHERE s.atom=a.id AND s.model=?1
                    )
             FROM atoms a
             LEFT JOIN vectors v ON v.atom=a.id
             ORDER BY a.first_seen,a.id",
        )?;
        let mut rows = statement.query([model_hash])?;
        let mut pending = Vec::new();
        let limit = limit.max(0) as usize;
        loop {
            if pending.len() >= limit {
                break;
            }
            let Some(row) = rows.next()? else {
                break;
            };
            if deadline.is_some_and(|(start, budget)| start.elapsed().as_millis() as u64 >= budget)
            {
                break;
            }
            let vector_model_matches = matches!(
                row.get_ref(3)?,
                rusqlite::types::ValueRef::Text(model) if model == model_hash.as_bytes()
            );
            let vector_blob = match row.get_ref(4)? {
                rusqlite::types::ValueRef::Blob(blob) => Some(blob),
                _ => None,
            };
            let topic_ready = row.get::<_, bool>(5)? || row.get::<_, bool>(6)?;
            let needs_vector =
                !vector_model_matches || !vector_blob.is_some_and(vector_blob_is_valid);
            let needs_topic = !topic_ready || needs_vector;
            if !needs_vector && !needs_topic {
                continue;
            }

            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let query: Option<String> = row.get(2)?;
            // Canonical atom identity includes observed title and query, so those inputs are
            // immutable for a retained atom. The refresh transaction rechecks them before
            // committing any work to close the read/encode/write race.
            let stored_vector = if needs_vector {
                None
            } else {
                vector_blob.map(decode_vector_blob)
            };
            pending.push(PendingAtom {
                id,
                title,
                query,
                stored_vector,
                needs_vector,
                needs_topic,
            });
        }
        Ok(pending)
    }
    #[cfg(test)]
    fn unindexed_atoms(
        &self,
        model_hash: &str,
        limit: i64,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        Ok(self
            .pending_atom_work(model_hash, limit, None)?
            .into_iter()
            .map(|row| (row.id, row.title, row.query))
            .collect())
    }
    fn activate_model(&mut self, model_hash: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='active_model_hash'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if previous.as_deref() != Some(model_hash) {
            tx.execute("DELETE FROM topics", [])?;
            tx.execute("DELETE FROM topic_skips", [])?;
            tx.execute("DELETE FROM vectors WHERE model<>?", [model_hash])?;
            tx.execute("INSERT INTO meta(key,value) VALUES('active_model_hash',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [model_hash])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn policy(&self, source: &str) -> Result<Policy> {
        let s: Option<String> = self
            .conn
            .query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                r.get(0)
            })
            .optional()?;
        s.map(|x| serde_json::from_str(&x).map_err(Into::into))
            .unwrap_or(Ok(Policy::default()))
    }
    pub fn set_policy(&mut self, source: &str, p: &Policy) -> Result<Value> {
        if p.selected_sites.len() + p.excluded_sites.len() > 500
            || p.selected_sites
                .iter()
                .chain(p.excluded_sites.iter())
                .any(|x| !policy::valid_site(x))
        {
            return Err(invalid("Invalid site policy."));
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stored: Option<String> = tx
            .query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                r.get(0)
            })
            .optional()?;
        let old: Policy = stored
            .map(|x| serde_json::from_str(&x))
            .transpose()?
            .unwrap_or_default();
        if p.capture_epoch == old.capture_epoch
            && serde_json::to_value(p)? == serde_json::to_value(&old)?
        {
            drop(tx);
            return self.status(source);
        }
        if p.capture_epoch <= old.capture_epoch {
            return Err(invalid("Policy epoch must increase."));
        }
        tx.execute("INSERT INTO sources(id,policy) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET policy=excluded.policy",params![source,serde_json::to_string(p)?])?;
        tx.execute("UPDATE site_rules SET excluded=0 WHERE source=?", [source])?;
        for site in &p.excluded_sites {
            tx.execute("INSERT INTO site_rules VALUES(?,?,?,1) ON CONFLICT(source,site) DO UPDATE SET epoch=max(epoch,excluded.epoch),excluded=1",params![source,site,p.capture_epoch])?;
        }
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='privacy'",
            [],
        )?;
        tx.commit()?;
        self.status(source)
    }
    pub fn status(&self, source: &str) -> Result<Value> {
        let (e, p) = self.generation()?;
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))?;
        let current_model = self.current_model_hash();
        let pending = self.pending_atoms_for_model(current_model.as_deref())?;
        let mut bytes = 0;
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        }
        Ok(
            json!({"protocol":1,"status":"ok","database_path":self.path,"evidence_generation":e,"privacy_generation":p,"atoms":count,"pending_atoms":pending,"vault_bytes":bytes,"policy":self.policy(source)?,"index_mode":if current_model.is_some(){"hybrid"}else{"lexical"},"model_available":current_model.is_some()}),
        )
    }
    pub fn ingest(&mut self, source: &str, epoch: u64, events: Vec<Event>) -> Result<Value> {
        if events.len() > 32 || serde_json::to_vec(&events)?.len() > 65536 {
            return Err(invalid("Batch exceeds its limit."));
        }
        for e in &events {
            e.validate()?
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let s: Option<String> = tx
            .query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                r.get(0)
            })
            .optional()?;
        let p: Policy = s
            .map(|x| serde_json::from_str(&x))
            .transpose()?
            .unwrap_or_default();
        let salt: String = tx.query_row(
            "SELECT value FROM meta WHERE key='canonical_salt'",
            [],
            |r| r.get(0),
        )?;
        let mut ack = vec![];
        let mut duplicate = vec![];
        let mut rejected = vec![];
        let mut added = 0;
        for raw in events {
            let eid = raw.event_id.clone();
            let reject = if epoch != p.capture_epoch {
                Some("STALE_CAPTURE_EPOCH")
            } else {
                None
            };
            if let Some(reason) = reject {
                rejected.push(json!({"id":eid,"reason":reason}));
                continue;
            }
            let rules = tx
                .prepare("SELECT site,epoch,excluded FROM site_rules WHERE source=?")?
                .query_map([source], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let relevant: Vec<_> = rules
                .iter()
                .filter(|(s, _, _)| policy::matches(&raw.site_key, s))
                .collect();
            let required = relevant.iter().map(|(_, e, _)| *e).max().unwrap_or(0);
            if raw.site_epoch < required || relevant.iter().any(|(_, _, x)| *x) {
                rejected.push(json!({"id":eid,"reason":"STALE_SITE_EPOCH"}));
                continue;
            }
            let e = match policy::allowed(&raw, &p) {
                Ok(x) => x,
                Err(reason) => {
                    rejected.push(json!({"id":eid,"reason":reason}));
                    continue;
                }
            };
            if tx
                .query_row(
                    "SELECT 1 FROM events WHERE source=? AND id=?",
                    params![source, eid],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                duplicate.push(eid);
                continue;
            }
            let canonical = hash(
                format!(
                    "{}\0{}\0{}\0{}\0{}\0{}",
                    salt,
                    source,
                    e.site_key,
                    e.kind,
                    e.title,
                    e.search_query.as_deref().unwrap_or("")
                )
                .as_bytes(),
            );
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM atoms WHERE canonical=?",
                    [&canonical],
                    |r| r.get(0),
                )
                .optional()?;
            let atom = existing.clone().unwrap_or_else(id);
            let count: i64 = tx.query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))?;
            let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
                + std::fs::metadata(format!("{}-wal", self.path.display()))
                    .map(|m| m.len())
                    .unwrap_or(0);
            if existing.is_none() && (count >= 10000 || bytes >= 96 * 1024 * 1024) {
                rejected.push(json!({"id":eid,"reason":"STORAGE_FULL"}));
                continue;
            }
            tx.execute("INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(canonical) DO UPDATE SET last_seen=max(last_seen,excluded.last_seen),seconds=min(3600,seconds+excluded.seconds)",params![atom,source,e.site_key,e.title,e.search_query,e.kind,e.observed_at,e.observed_at,e.foreground_seconds,canonical])?;
            tx.execute(
                "INSERT INTO events VALUES(?,?,?,?,?)",
                params![source, eid, atom, e.visit_id, e.observed_at],
            )?;
            let dt = chrono::DateTime::parse_from_rfc3339(&e.observed_at).unwrap();
            let day = dt.format("%Y-%m-%d").to_string();
            let session = format!("{}:{}:{}", source, epoch, dt.timestamp() / 1800);
            let mass = if e.kind == "search" {
                1.0
            } else {
                (0.25 + 0.5 * e.foreground_seconds as f64 / 60.0).min(0.75)
            };
            tx.execute("INSERT INTO atom_days VALUES(?,?,?,?) ON CONFLICT(atom,day,session) DO UPDATE SET mass=max(mass,excluded.mass)",params![atom,day,session,mass])?;
            ack.push(eid);
            added += 1;
        }
        if added > 0 {
            tx.execute(
                "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='evidence'",
                [],
            )?;
        }
        tx.execute("DELETE FROM atoms WHERE last_seen < strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') AND id NOT IN (SELECT atom FROM feedback WHERE action='confirm_constraint')",[])?;
        tx.execute("DELETE FROM events WHERE time < strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days') OR rowid IN (SELECT rowid FROM events ORDER BY time DESC LIMIT -1 OFFSET 20000)",[])?;
        tx.execute("DELETE FROM atom_days WHERE rowid IN (SELECT rowid FROM atom_days ORDER BY day DESC LIMIT -1 OFFSET 30000)",[])?;
        tx.commit()?;
        let mut r = self.status(source)?;
        r["acknowledged_ids"] = json!(ack);
        r["duplicate_ids"] = json!(duplicate);
        r["rejected"] = json!(rejected);
        Ok(r)
    }
    pub fn forget(
        &mut self,
        source: &str,
        site: Option<&str>,
        atom: Option<&str>,
        epoch: u64,
    ) -> Result<Value> {
        if let Some(s) = site {
            if !policy::valid_site(s) {
                return Err(invalid("Invalid hostname."));
            }
        }
        if let Some(a) = atom {
            check_id(a)?
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut count = 0;
        if let Some(site) = site {
            let old: u64 = tx
                .query_row(
                    "SELECT epoch FROM site_rules WHERE source=? AND site=?",
                    params![source, site],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            if epoch < old {
                return Ok(json!({"status":"ok","superseded":true,"deleted":0}));
            }
            tx.execute("INSERT INTO site_rules VALUES(?,?,?,1) ON CONFLICT(source,site) DO UPDATE SET epoch=max(epoch,excluded.epoch),excluded=1",params![source,site,epoch])?;
            count = tx.execute(
                "DELETE FROM atoms WHERE source=? AND (site=? OR site LIKE ?)",
                params![source, site, format!("%.{site}")],
            )?
        } else if let Some(a) = atom {
            count = tx.execute(
                "DELETE FROM atoms WHERE id=? AND source=?",
                params![a, source],
            )?
        } else {
            count += tx.execute("DELETE FROM atoms WHERE source=?", [source])?;
            let s: String =
                tx.query_row("SELECT policy FROM sources WHERE id=?", [source], |r| {
                    r.get(0)
                })?;
            let mut p: Policy = serde_json::from_str(&s)?;
            if !p.paused {
                p.capture_epoch = p.capture_epoch.max(epoch) + 1;
            }
            p.paused = true;
            tx.execute(
                "UPDATE sources SET policy=? WHERE id=?",
                params![serde_json::to_string(&p)?, source],
            )?;
        }
        tx.execute("DELETE FROM receipts", [])?;
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key IN ('evidence','privacy')",
            [],
        )?;
        tx.commit()?;
        Ok(
            json!({"status":"ok","deleted":count,"privacy_generation":self.generation()?.1,"logical_deletion":true}),
        )
    }
    pub fn feedback(
        &mut self,
        source: &str,
        atom: &str,
        action: &str,
        text: Option<&str>,
    ) -> Result<Value> {
        check_id(atom)?;
        if ![
            "not_about_me",
            "wrong_topic",
            "temporary_research",
            "confirm_constraint",
            "do_not_use",
        ]
        .contains(&action)
        {
            return Err(invalid("Unknown correction."));
        }
        if action == "confirm_constraint"
            && (text.is_none()
                || text.unwrap().trim().is_empty()
                || text.unwrap().chars().count() > 512
                || policy::sensitive(text.unwrap()))
        {
            return Err(invalid("Provide a non-sensitive explicit constraint."));
        }
        if self
            .conn
            .query_row(
                "SELECT 1 FROM atoms WHERE id=? AND source=?",
                params![atom, source],
                |_| Ok(()),
            )
            .optional()?
            .is_none()
        {
            return Err(invalid("Evidence not found."));
        }
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM feedback WHERE action='confirm_constraint'",
            [],
            |r| r.get(0),
        )?;
        if action == "confirm_constraint" && count >= 1000 {
            return Err(Error(
                "STORAGE_FULL",
                "Remove an existing confirmed constraint first.".into(),
            ));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO feedback(id,atom,action,text,time) VALUES(?,?,?,?,?)",
            params![
                id(),
                atom,
                action,
                text.map(|x| policy::clean(x, 512)),
                now()
            ],
        )?;
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='privacy'",
            [],
        )?;
        tx.commit()?;
        Ok(json!({"status":"ok"}))
    }
    pub fn dashboard(&self, source: &str) -> Result<Value> {
        if !self.conn.is_autocommit() {
            return self.dashboard_with_snapshot(source);
        }
        // Keep the evidence, policy, activity-label refresh, and grouped
        // dashboard view on one writer-serialized snapshot. A concurrent
        // policy narrowing must not leave old-site candidates in the response.
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        match self.dashboard_with_snapshot(source) {
            Ok(value) => {
                tx.commit()?;
                Ok(value)
            }
            Err(error) => {
                drop(tx);
                Err(error)
            }
        }
    }

    fn dashboard_with_snapshot(&self, source: &str) -> Result<Value> {
        const DASHBOARD_CARD_LIMIT: i64 = 60;
        // The vault refuses new atoms at 10,000, so this fixed bound covers
        // every retained current-model candidate without an unbounded read.
        const DASHBOARD_RESEARCH_CANDIDATE_LIMIT: i64 = 10_000;
        let mut cards = vec![];
        let mut stmt=self.conn.prepare("SELECT a.id,a.site,a.title,a.query,a.kind,a.seconds,a.last_seen,(SELECT count(DISTINCT session) FROM atom_days WHERE atom=a.id) FROM atoms a WHERE source=? AND (last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') OR id IN (SELECT atom FROM feedback WHERE action='confirm_constraint')) ORDER BY last_seen DESC,a.id LIMIT ?")?;
        let card_rows = stmt
            .query_map(params![source, DASHBOARD_CARD_LIMIT], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let model_hash = self.current_model_hash();
        let candidate_rows = if let Some(model_hash) = model_hash.as_deref() {
            let mut stmt = self.conn.prepare("SELECT a.id,a.site,a.title,a.query,a.kind,a.seconds,a.last_seen,(SELECT count(DISTINCT session) FROM atom_days WHERE atom=a.id) FROM atoms a WHERE a.source=? AND (a.last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') OR a.id IN (SELECT atom FROM feedback WHERE action='confirm_constraint')) AND EXISTS (SELECT 1 FROM atom_topics m JOIN topics t ON t.id=m.topic WHERE m.atom=a.id AND t.model=?) ORDER BY a.last_seen DESC,a.id LIMIT ?")?;
            let rows = stmt.query_map(
                params![source, model_hash, DASHBOARD_RESEARCH_CANDIDATE_LIMIT],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                    ))
                },
            )?;
            let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        } else {
            Vec::new()
        };

        let dashboard_policy = self.policy(source)?;
        let mut groupable_candidates: Vec<_> = candidate_rows
            .into_iter()
            .filter(|(_, site, title, query, _, _, _, _)| {
                importance::groupable(title, query.as_deref())
                    && !dashboard_policy
                        .excluded_sites
                        .iter()
                        .any(|rule| policy::matches(site, rule))
                    && (!dashboard_policy.selected_only
                        || dashboard_policy
                            .selected_sites
                            .iter()
                            .any(|rule| policy::matches(site, rule)))
            })
            .collect();
        let mut correction_ids: Vec<_> = card_rows
            .iter()
            .map(|row| row.0.clone())
            .chain(
                groupable_candidates
                    .iter()
                    .map(|(id, _, _, _, _, _, _, _)| id.clone()),
            )
            .collect();
        correction_ids.sort();
        correction_ids.dedup();
        let feedback_by_atom = self.corrections_for_atoms(&correction_ids)?;
        groupable_candidates.retain(|(id, _, _, _, _, _, _, _)| {
            !feedback_by_atom
                .get(id)
                .is_some_and(|state| state.memory_excluded)
        });

        let mut groupable_ids: Vec<_> = groupable_candidates
            .iter()
            .map(|(id, _, _, _, _, _, _, _)| id.clone())
            .collect();
        groupable_ids.sort();
        groupable_ids.dedup();
        let mut card_by_id = HashMap::new();
        for (id, site, title, query, kind, seconds, time, sessions) in card_rows {
            let feedback = feedback_by_atom.get(&id).cloned().unwrap_or_default();
            let confirmed = feedback.latest_confirmation.is_some();
            let text = feedback
                .latest_confirmation
                .as_ref()
                .map(|entry| json!(entry.text.clone()))
                .unwrap_or_else(|| json!(query.as_deref().unwrap_or(&title)));
            let corrections = feedback.corrections_json();
            let prominent =
                importance::prominent(&title, query.as_deref(), seconds, sessions, confirmed);
            let prominent = prominent && !feedback.memory_excluded;
            let card = json!({"id":id,"site":site,"text":text,"state":if confirmed{"confirmed"}else{"observed"},"last_seen":time,"sessions":sessions,"sites":1,"kind":kind,"prominent":prominent,"corrections":corrections});
            card_by_id.insert(id, card.clone());
            cards.push(card);
        }
        let mut r = self.status(source)?;
        let cache_headroom = topic_cache_headroom_bytes(&self.conn, &self.path)?;
        let mut topics = inference::activity_with_cache_headroom_for_atoms(
            &self.conn,
            model_hash.as_deref(),
            cache_headroom,
            &groupable_ids,
        )?;
        let mut membership = HashMap::<String, String>::new();
        if let Some(model_hash) = model_hash.filter(|_| !groupable_ids.is_empty()) {
            let placeholders = vec!["?"; groupable_ids.len()].join(",");
            let sql = format!(
                "SELECT m.atom,m.topic,m.mass FROM atom_topics m JOIN topics t ON t.id=m.topic WHERE t.model=? AND m.atom IN ({placeholders}) ORDER BY m.atom,m.mass DESC,m.topic"
            );
            let params = std::iter::once(model_hash.as_str())
                .chain(groupable_ids.iter().map(String::as_str));
            for row in self
                .conn
                .prepare(&sql)?
                .query_map(rusqlite::params_from_iter(params), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
            {
                let (atom, topic) = row?;
                membership.entry(atom).or_insert(topic);
            }
        }
        let candidate_kinds: HashMap<_, _> = groupable_candidates
            .iter()
            .map(|(id, _, _, _, kind, _, _, _)| (id.clone(), kind.clone()))
            .collect();
        let mut sessions_by_topic = HashMap::<String, HashSet<String>>::new();
        let mut items_by_topic_session = HashMap::<(String, String), HashSet<String>>::new();
        let mut searches_by_topic_session = HashMap::<(String, String), HashSet<String>>::new();
        if !groupable_ids.is_empty() {
            let placeholders = vec!["?"; groupable_ids.len()].join(",");
            let sql = format!("SELECT atom,session FROM atom_days WHERE atom IN ({placeholders})");
            let params = groupable_ids.iter().map(String::as_str);
            for row in self
                .conn
                .prepare(&sql)?
                .query_map(rusqlite::params_from_iter(params), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
            {
                let (atom, session) = row?;
                let Some(topic) = membership.get(&atom) else {
                    continue;
                };
                sessions_by_topic
                    .entry(topic.clone())
                    .or_default()
                    .insert(session.clone());
                let key = (topic.clone(), session);
                items_by_topic_session
                    .entry(key.clone())
                    .or_default()
                    .insert(atom.clone());
                if candidate_kinds
                    .get(&atom)
                    .is_some_and(|kind| kind == "search")
                {
                    searches_by_topic_session
                        .entry(key)
                        .or_default()
                        .insert(atom);
                }
            }
        }
        let mut by_topic = HashMap::<String, Vec<Value>>::new();
        let mut eligible_titles = HashMap::<String, String>::new();
        for (id, site, title, query, kind, seconds, time, sessions) in groupable_candidates {
            eligible_titles.insert(id.clone(), title.clone());
            let card = if let Some(card) = card_by_id.get(&id) {
                card.clone()
            } else {
                let feedback = feedback_by_atom.get(&id).cloned().unwrap_or_default();
                let confirmed = feedback.latest_confirmation.is_some();
                let text = feedback
                    .latest_confirmation
                    .as_ref()
                    .map(|entry| json!(entry.text.clone()))
                    .unwrap_or_else(|| json!(query.as_deref().unwrap_or(&title)));
                let corrections = feedback.corrections_json();
                let prominent =
                    importance::prominent(&title, query.as_deref(), seconds, sessions, confirmed);
                let prominent = prominent && !feedback.memory_excluded;
                json!({"id":id,"site":site,"text":text,"state":if confirmed{"confirmed"}else{"observed"},"last_seen":time,"sessions":sessions,"sites":1,"kind":kind,"prominent":prominent,"corrections":corrections})
            };
            if let Some(topic) = membership.get(&id) {
                by_topic.entry(topic.clone()).or_default().push(card);
            }
        }
        let source_topic_labels: HashMap<_, _> = by_topic
            .iter()
            .map(|(topic, items)| {
                let label = items
                    .iter()
                    .filter_map(|item| item["text"].as_str())
                    .find(|text| importance::groupable(text, None))
                    .or_else(|| {
                        items
                            .iter()
                            .filter(|item| item["state"] == "observed")
                            .filter_map(|item| item["id"].as_str())
                            .filter_map(|id| eligible_titles.get(id))
                            .find(|title| importance::groupable(title, None))
                            .map(String::as_str)
                    })
                    .unwrap_or("Research")
                    .chars()
                    .take(72)
                    .collect::<String>();
                (topic.clone(), label)
            })
            .collect();
        if let Some(topics) = topics.as_array_mut() {
            for topic in topics {
                let Some(topic_id) = topic["id"].as_str() else {
                    continue;
                };
                topic["label"] = json!(source_topic_labels
                    .get(topic_id)
                    .cloned()
                    .unwrap_or_else(|| "Research".to_owned()));
            }
        }
        let mut memories = Vec::new();
        if let Some(topic_list) = topics.as_array() {
            for topic in topic_list {
                let Some(topic_id) = topic["id"].as_str() else {
                    continue;
                };
                let Some(mut items) = by_topic.remove(topic_id) else {
                    continue;
                };
                if items.len() < 2 {
                    continue;
                }
                let independent_sessions = sessions_by_topic
                    .get(topic_id)
                    .is_some_and(|sessions| sessions.len() >= 2);
                let coherent_search_session = items_by_topic_session.iter().any(
                    |((candidate_topic, session), session_items)| {
                        candidate_topic == topic_id
                            && searches_by_topic_session
                                .get(&(candidate_topic.clone(), session.clone()))
                                .is_some_and(|searches| {
                                    session_items.difference(searches).count() >= 2
                                })
                    },
                );
                if !independent_sessions && !coherent_search_session {
                    continue;
                }
                let sites: HashSet<_> = items
                    .iter()
                    .filter_map(|item| item["site"].as_str())
                    .map(str::to_owned)
                    .collect();
                items.sort_by(|a, b| {
                    b["last_seen"]
                        .as_str()
                        .cmp(&a["last_seen"].as_str())
                        .then(a["id"].as_str().cmp(&b["id"].as_str()))
                });
                let last_seen = items[0]["last_seen"].clone();
                let evidence_count = items.len();
                // A topic's stored label may have come from an observation the
                // user later corrected. Derive display text only from the
                // still-eligible evidence in this memory.
                let label = source_topic_labels
                    .get(topic_id)
                    .cloned()
                    .unwrap_or_else(|| "Research".to_owned());
                items.truncate(5);
                memories.push(json!({
                    "id": topic_id,
                    "label": label,
                    "last_seen": last_seen,
                    "sessions": sessions_by_topic.get(topic_id).map_or(0, HashSet::len),
                    "sites": sites.len(),
                    "evidence_count": evidence_count,
                    "items": items,
                }));
            }
        }
        memories.sort_by(|a, b| {
            b["last_seen"]
                .as_str()
                .cmp(&a["last_seen"].as_str())
                .then(a["id"].as_str().cmp(&b["id"].as_str()))
        });
        memories.truncate(8);
        r["cards"] = json!(cards);
        r["topics"] = topics;
        r["memories"] = json!(memories);
        Ok(r)
    }
    fn corrections(&self, id: &str) -> Result<FeedbackResolution> {
        let ids = [id.to_string()];
        Ok(feedback::load_for_atoms(&self.conn, &ids)?
            .remove(id)
            .unwrap_or_default())
    }
    fn corrections_for_atoms(&self, ids: &[String]) -> Result<HashMap<String, FeedbackResolution>> {
        feedback::load_for_atoms(&self.conn, ids)
    }
    pub fn refresh(&mut self, model_path: &Path, budget: u64) -> Result<Value> {
        let encoder = match model::Encoder::open(model_path) {
            Ok(m) => m,
            Err(_) => {
                return Ok(
                    json!({"status":"partial","mode":"lexical","reason":"MODEL_UNAVAILABLE"}),
                );
            }
        };
        self.refresh_with_encoder(&encoder, budget)
    }
    fn refresh_with_encoder(&mut self, encoder: &model::Encoder, budget: u64) -> Result<Value> {
        let model_hash = encoder.manifest.model_hash.clone();
        self.refresh_with_encode_fn(&model_hash, budget, |text| encoder.encode(text))
    }
    fn refresh_with_encode_fn<F>(
        &mut self,
        model_hash: &str,
        budget: u64,
        mut encode: F,
    ) -> Result<Value>
    where
        F: FnMut(&str) -> Option<Vec<f32>>,
    {
        let start = Instant::now();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path.with_extension("refresh.lock"))?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return Ok(json!({"status":"partial","reason":"REFRESH_BUSY"}));
        }
        self.activate_model(model_hash)?;
        let generation = self.generation()?;
        let rows = self.pending_atom_work(model_hash, 256, Some((start, budget)))?;
        let mut processed = 0;
        for row in rows {
            if start.elapsed().as_millis() as u64 >= budget {
                break;
            }
            if !row.needs_vector && !row.needs_topic {
                continue;
            }

            let vector = if row.needs_vector {
                let title_vector = encode(&row.title);
                let query_vector = row.query.as_deref().and_then(&mut encode);
                match (title_vector, query_vector) {
                    (Some(title), Some(query)) => model::normalize(
                        title
                            .iter()
                            .zip(query)
                            .map(|(title, query)| {
                                algorithm::TITLE_WEIGHT * title + algorithm::QUERY_WEIGHT * query
                            })
                            .collect(),
                    ),
                    (title, query) => title.or(query),
                }
            } else {
                row.stored_vector.clone()
            };
            let Some(vector) = vector.filter(|vector| {
                vector.len() == algorithm::DIMENSIONS && vector.iter().all(|x| x.is_finite())
            }) else {
                continue;
            };

            // Encoding happens outside the write transaction. Recheck every live input before
            // committing so a concurrent privacy change, deletion, text change, or model switch
            // cannot recreate stale vector/topic state.
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_privacy: i64 = tx
                .query_row("SELECT value FROM meta WHERE key='privacy'", [], |r| {
                    r.get::<_, String>(0)
                })?
                .parse()
                .unwrap_or(-1);
            if current_privacy != generation.1 {
                drop(tx);
                break;
            }
            let active_model: Option<String> = tx
                .query_row(
                    "SELECT value FROM meta WHERE key='active_model_hash'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            if active_model.as_deref() != Some(model_hash) {
                drop(tx);
                break;
            }
            let live_input: Option<(String, Option<String>)> = tx
                .query_row("SELECT title,query FROM atoms WHERE id=?", [&row.id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if live_input
                .as_ref()
                .is_none_or(|(title, query)| title != &row.title || query != &row.query)
            {
                drop(tx);
                continue;
            }

            if row.needs_vector {
                let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
                tx.execute(
                    "INSERT OR REPLACE INTO vectors VALUES(?,?,?)",
                    params![row.id, model_hash, bytes],
                )?;
            }
            let cache_headroom = topic_cache_headroom_bytes(&tx, &self.path)?;
            inference::assign_with_cache_headroom(
                &tx,
                &row.id,
                &vector,
                row.query.as_deref().unwrap_or(&row.title),
                model_hash,
                cache_headroom,
            )?;
            tx.commit()?;
            processed += 1;
        }
        let pending = self.pending_atoms_for_model(Some(model_hash))?;
        Ok(
            json!({"status":if pending>0{"partial"}else{"ok"},"processed":processed,"pending_atoms":pending,"mode":"hybrid"}),
        )
    }
    pub fn recall(&mut self, r: &Recall, source: &str, model_path: &Path) -> Result<Value> {
        r.validate()?;
        let start = Instant::now();
        let budget = r.budget_ms.clamp(250, 2000);
        let p = self.policy(source)?;
        if !p.recall_enabled || !p.consent {
            return Ok(
                json!({"protocol":1,"request_id":r.request_id,"status":"blocked","context":[],"warnings":["Assistant recall is disabled in Privacy."]}),
            );
        }
        let encoder = model::Encoder::open(model_path).ok();
        let remaining = budget.saturating_sub(start.elapsed().as_millis() as u64);
        let refresh_budget = (budget / 4).min(250).min(remaining / 2);
        let refresh = match encoder.as_ref() {
            Some(encoder) => self.refresh_with_encoder(encoder, refresh_budget)?,
            None => json!({"status":"partial","mode":"lexical","reason":"MODEL_UNAVAILABLE"}),
        };
        // If startup or refresh consumed the request window, report the
        // retrieval path actually used instead of claiming a hybrid search.
        let encoder =
            encoder.filter(|_| (start.elapsed().as_millis() as u64) < budget.saturating_sub(25));
        let generation = self.generation()?;
        let candidate_batch = crate::retrieval::candidates(
            &self.conn,
            source,
            &p,
            r,
            encoder.as_ref(),
            start,
            budget,
        )?;
        let compare_models = candidate_batch.compare_models;
        let candidates = candidate_batch.candidates;
        let mut packet = json!({"protocol":1,"request_id":r.request_id,"status":"partial","as_of":now(),"generation":{"evidence":generation.0,"privacy":generation.1},"index":{"mode":if encoder.is_some(){"hybrid"}else{"lexical"},"pending_atoms":self.status(source)?["pending_atoms"]},"context":[],"alternatives":[],"warnings":if encoder.is_none(){vec!["Semantic model unavailable; lexical matching only."]}else{vec![]}});
        // Preserve the rank position of each duplicate group while keeping
        // every feasible real representative in its original rank order.
        // Confirmed records intentionally have no payload key and remain
        // distinct by atom and confirmation provenance.
        let mut packet_groups: Vec<Vec<crate::retrieval::RankedCandidate>> = Vec::new();
        let mut observed_group_indices =
            HashMap::<crate::retrieval::ObservedPayloadKey, usize>::new();
        for candidate in candidates {
            if let Some(key) = candidate.observed_payload_key.as_ref() {
                if let Some(index) = observed_group_indices.get(key).copied() {
                    packet_groups[index].push(candidate);
                    continue;
                }
                let index = packet_groups.len();
                observed_group_indices.insert(key.clone(), index);
                packet_groups.push(vec![candidate]);
            } else {
                packet_groups.push(vec![candidate]);
            }
        }

        let try_add_candidate = |packet: &mut Value,
                                 counts: &mut HashMap<String, u32>,
                                 candidate: &crate::retrieval::RankedCandidate|
         -> Result<bool> {
            if *counts.get(&candidate.site).unwrap_or(&0) >= algorithm::MAX_RECORDS_PER_SITE {
                return Ok(false);
            }
            let record = candidate.record.clone();
            packet["context"].as_array_mut().unwrap().push(record);
            if serde_json::to_vec(packet)?.len() + 64 > r.max_bytes {
                packet["context"].as_array_mut().unwrap().pop();
                return Ok(false);
            }
            *counts.entry(candidate.site.clone()).or_default() += 1;
            Ok(true)
        };

        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut selected_groups = HashSet::new();
        let mut emitted_arms = 0u8;
        if compare_models {
            // Allocate one feasible distinct payload to each still-uncovered
            // comparison arm before filling in global rank order. Group and
            // representative iteration retain the fused score/ID ordering.
            loop {
                if packet["context"].as_array().unwrap().len() >= algorithm::MAX_CONTEXT_RECORDS {
                    break;
                }
                let uncovered = 0b11 & !emitted_arms;
                let mut selected = None;
                'groups: for (group_index, group) in packet_groups.iter().enumerate() {
                    if selected_groups.contains(&group_index) {
                        continue;
                    }
                    for candidate in group {
                        if candidate.comparison_arms & uncovered == 0 {
                            continue;
                        }
                        if try_add_candidate(&mut packet, &mut counts, candidate)? {
                            selected = Some((group_index, candidate.comparison_arms));
                            break 'groups;
                        }
                    }
                }
                let Some((group_index, arms)) = selected else {
                    break;
                };
                selected_groups.insert(group_index);
                emitted_arms |= arms;
                if emitted_arms == 0b11 {
                    break;
                }
            }
        }

        // Once arm coverage is exhausted (or for a single-target request),
        // fill the remaining packet slots in the ordinary global rank order.
        for (group_index, group) in packet_groups.iter().enumerate() {
            if selected_groups.contains(&group_index) {
                continue;
            }
            if packet["context"].as_array().unwrap().len() >= algorithm::MAX_CONTEXT_RECORDS {
                break;
            }
            for candidate in group {
                if try_add_candidate(&mut packet, &mut counts, candidate)? {
                    selected_groups.insert(group_index);
                    emitted_arms |= candidate.comparison_arms;
                    break;
                }
            }
        }

        if compare_models
            && !packet["context"].as_array().unwrap().is_empty()
            && emitted_arms != 0b11
        {
            packet["context"][0]["limits"]
                .as_array_mut()
                .expect("recall records include limits")
                .push(json!(
                    "Comparison evidence covers only one requested model."
                ));
        }
        let empty = packet["context"].as_array().unwrap().is_empty();
        let deadline_reached = start.elapsed().as_millis() as u64 >= budget;
        packet["status"] = json!(if deadline_reached {
            "partial"
        } else if empty {
            "empty"
        } else if encoder.is_none() || refresh["status"] != "ok" {
            "partial"
        } else {
            "ok"
        });
        let receipt_tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current_privacy: i64 = receipt_tx.query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='privacy'",
            [],
            |r| r.get(0),
        )?;
        if current_privacy != generation.1 {
            packet["context"] = json!([]);
            packet["status"] = json!("blocked");
            packet["warnings"] = json!(["Privacy changed during recall. Retry."])
        }
        let ids: Vec<Value> = packet["context"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["id"].clone())
            .collect();
        let size = serde_json::to_vec(&packet)?.len();
        if size > r.max_bytes {
            packet = json!({"protocol":1,"request_id":r.request_id,"status":"empty","context":[],"warnings":["Packet budget too small."]})
        }
        receipt_tx.execute(
            "INSERT OR REPLACE INTO receipts VALUES(?,?,?,?,?,?,?)",
            params![
                r.request_id,
                serde_json::to_string(&ids)?,
                generation.0,
                generation.1,
                serde_json::to_vec(&packet)?.len(),
                now(),
                packet["status"].as_str()
            ],
        )?;
        receipt_tx.execute("DELETE FROM receipts WHERE time<strftime('%Y-%m-%dT%H:%M:%SZ','now','-7 days') OR rowid IN (SELECT rowid FROM receipts ORDER BY time DESC LIMIT -1 OFFSET 1000)",[])?;
        receipt_tx.commit()?;
        Ok(packet)
    }
    pub fn receipts(&self) -> Result<Value> {
        let rows=self.conn.prepare("SELECT id,ids,evidence,privacy,bytes,time,status FROM receipts ORDER BY time DESC LIMIT 100")?.query_map([],|r|Ok(json!({"request_id":r.get::<_,String>(0)?,"ids":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(json!([])),"evidence_generation":r.get::<_,i64>(2)?,"privacy_generation":r.get::<_,i64>(3)?,"bytes":r.get::<_,i64>(4)?,"time":r.get::<_,String>(5)?,"status":r.get::<_,String>(6)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(json!({"status":"ok","receipts":rows}))
    }
}

impl Vault {
    pub fn explain(&self, source: &str, ids: &[String], max_bytes: usize) -> Result<Value> {
        if ids.len() > 6 || !(512..=4096).contains(&max_bytes) {
            return Err(invalid(
                "Explain accepts at most six evidence IDs and 512–4096 bytes.",
            ));
        }
        let p = self.policy(source)?;
        if !p.recall_enabled || !p.consent {
            return Ok(json!({"status":"blocked","evidence":[]}));
        }
        let generation = self.generation()?;
        let mut packet = json!({"protocol":1,"status":"ok","generation":{"evidence":generation.0,"privacy":generation.1},"evidence":[]});
        for id in ids {
            check_id(id)?;
            let row:Option<(String,String,Option<String>,String)>=self.conn.query_row("SELECT site,title,query,last_seen FROM atoms WHERE id=? AND source=? AND (last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days') OR id IN (SELECT atom FROM feedback WHERE action='confirm_constraint'))",params![id,source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((site, title, query, time)) = row {
                if p.excluded_sites.iter().any(|r| policy::matches(&site, r))
                    || p.selected_only
                        && !p.selected_sites.iter().any(|r| policy::matches(&site, r))
                {
                    continue;
                }
                let corrections = self.corrections(id)?;
                if corrections.recall_suppressed {
                    continue;
                }
                packet["evidence"].as_array_mut().unwrap().push(json!({"id":id,"site":site,"title":title,"search_query":query,"last_seen":time,"corrections":corrections.corrections_json(),"source_type":"untrusted_browser_metadata"}));
                if serde_json::to_vec(&packet)?.len() > max_bytes {
                    packet["evidence"].as_array_mut().unwrap().pop();
                    break;
                }
            }
        }
        if self.generation()?.1 != generation.1 {
            packet["evidence"] = json!([]);
            packet["status"] = json!("blocked")
        };
        Ok(packet)
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(&dir.path().join("vault.sqlite")).unwrap();
        (dir, vault)
    }

    #[test]
    fn cache_headroom_counts_database_and_wal_inside_existing_96_mib_gate() {
        const STORAGE_LIMIT: u64 = 96 * 1024 * 1024;
        const RESERVE: u64 = 64 * 4096;
        let (dir, vault) = vault();
        let database = dir.path().join("sized-database.sqlite");
        std::fs::File::create(&database)
            .unwrap()
            .set_len(STORAGE_LIMIT - RESERVE - 1024)
            .unwrap();
        assert_eq!(
            topic_cache_headroom_bytes(&vault.conn, &database).unwrap(),
            1024
        );

        let mut wal_name = database.as_os_str().to_os_string();
        wal_name.push("-wal");
        std::fs::File::create(PathBuf::from(wal_name))
            .unwrap()
            .set_len(2048)
            .unwrap();
        assert_eq!(
            topic_cache_headroom_bytes(&vault.conn, &database).unwrap(),
            0,
            "database and WAL together must not exceed 96 MiB minus the existing reserve"
        );
    }

    #[test]
    fn small_page_vault_uses_reference_refresh_without_changing_page_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("small-pages.sqlite");
        {
            let bootstrap = Connection::open(&database).unwrap();
            bootstrap
                .execute_batch("PRAGMA page_size=512; CREATE TABLE page_size_bootstrap(id INTEGER PRIMARY KEY);")
                .unwrap();
            let page_size: i64 = bootstrap
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .unwrap();
            assert_eq!(page_size, 512);
        }

        let mut vault = Vault::open(&database).unwrap();
        let page_size: i64 = vault
            .conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(page_size, 512);
        assert_eq!(
            topic_cache_headroom_bytes(&vault.conn, &database).unwrap(),
            0
        );
        vault
            .set_policy(
                "source",
                &Policy {
                    consent: true,
                    recall_enabled: true,
                    capture_epoch: 1,
                    ..Policy::default()
                },
            )
            .unwrap();
        let events = (0..2)
            .map(|index| Event {
                event_id: id(),
                visit_id: id(),
                site_key: "example.com".to_owned(),
                site_epoch: 0,
                observed_at: now(),
                kind: "visit".to_owned(),
                title: format!("Desk lamp installation {index}"),
                search_query: None,
                foreground_seconds: 30,
            })
            .collect();
        let ingest = vault.ingest("source", 1, events).unwrap();
        assert_eq!(ingest["acknowledged_ids"].as_array().unwrap().len(), 2);

        let refreshed = vault
            .refresh_with_encode_fn("model", 60_000, |_| Some(axis(0)))
            .unwrap();
        assert_eq!(refreshed["processed"], 2);
        assert_eq!(refreshed["pending_atoms"], 0);
        for (table, expected) in [
            ("vectors", 2),
            ("topics", 1),
            ("atom_topics", 2),
            ("topic_cache", 0),
            ("topic_cache_buckets", 0),
        ] {
            let count: i64 = vault
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "unexpected {table} rows before reopen");
        }
        drop(vault);

        let reopened = Vault::open(&database).unwrap();
        let page_size: i64 = reopened
            .conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(page_size, 512);
        for (table, expected) in [
            ("vectors", 2),
            ("topics", 1),
            ("atom_topics", 2),
            ("topic_cache", 0),
            ("topic_cache_buckets", 0),
        ] {
            let count: i64 = reopened
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "unexpected {table} rows after reopen");
        }
    }

    fn downgrade_feedback_to_schema3(vault: &Vault) {
        vault
            .conn
            .execute_batch(
                "DROP TRIGGER IF EXISTS topic_cache_atom_days_insert;
                 DROP TRIGGER IF EXISTS topic_cache_atom_days_update;
                 DROP TRIGGER IF EXISTS topic_cache_atom_days_delete;
                 DROP TRIGGER IF EXISTS topic_cache_atom_topics_insert;
                 DROP TRIGGER IF EXISTS topic_cache_atom_topics_update;
                 DROP TRIGGER IF EXISTS topic_cache_atom_topics_delete;
                 DROP TRIGGER IF EXISTS topic_cache_vectors_insert;
                 DROP TRIGGER IF EXISTS topic_cache_vectors_update;
                 DROP TRIGGER IF EXISTS topic_cache_vectors_delete;
                 DROP TRIGGER IF EXISTS topic_cache_feedback_insert;
                 DROP TRIGGER IF EXISTS topic_cache_feedback_update;
                 DROP TRIGGER IF EXISTS topic_cache_feedback_delete;
                 DROP TRIGGER IF EXISTS topic_cache_bucket_insert;
                 DROP TRIGGER IF EXISTS topic_cache_bucket_update;
                 DROP TRIGGER IF EXISTS topic_cache_bucket_delete;
                 DROP TRIGGER IF EXISTS topic_cache_atoms_update;
                 DROP TABLE IF EXISTS topic_cache_buckets;
                 DROP TABLE IF EXISTS topic_cache;
                 DROP TABLE feedback;
                 CREATE TABLE feedback(
                     id TEXT PRIMARY KEY,
                     atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,
                     action TEXT NOT NULL,
                     text TEXT,
                     time TEXT NOT NULL
                 );",
            )
            .unwrap();
    }

    fn insert_atom(v: &Vault, atom: &str, first_seen: &str, vector: &[f32], model: &str) {
        v.conn
            .execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![
                    atom,
                    "source",
                    "example.com",
                    format!("Title {atom}"),
                    Option::<String>::None,
                    "visit",
                    first_seen,
                    now(),
                    30,
                    atom,
                ],
            )
            .unwrap();
        v.conn
            .execute(
                "INSERT INTO atom_days VALUES(?,?,?,?)",
                params![atom, chrono::Utc::now().date_naive().to_string(), atom, 1.0,],
            )
            .unwrap();
        let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
        v.conn
            .execute(
                "INSERT INTO vectors VALUES(?,?,?)",
                params![atom, model, bytes],
            )
            .unwrap();
    }

    fn axis(index: usize) -> Vec<f32> {
        let mut vector = vec![0.0; 256];
        vector[index] = 1.0;
        vector
    }

    #[test]
    fn status_pending_atoms_tracks_active_model() {
        let (_dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "old");
        inference::assign(&v.conn, "a", &axis(0), "Old", "old").unwrap();
        assert_eq!(v.pending_atoms_for_model(Some("old")).unwrap(), 0);
        assert_eq!(v.pending_atoms_for_model(Some("new")).unwrap(), 1);
    }

    #[test]
    fn model_change_leaves_no_old_model_topic_assignments() {
        let (_dir, mut v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "old");
        v.activate_model("old").unwrap();
        inference::assign(&v.conn, "a", &axis(0), "Old label", "old").unwrap();
        assert_eq!(
            inference::activity(&v.conn, Some("old"))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(inference::activity(&v.conn, Some("new"))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        v.activate_model("new").unwrap();
        for table in [
            "topics",
            "atom_topics",
            "vectors",
            "topic_cache",
            "topic_cache_buckets",
        ] {
            let count: i64 = v
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        assert_eq!(v.pending_atoms_for_model(Some("new")).unwrap(), 1);
    }

    #[test]
    fn topic_rebuild_is_deterministic_given_same_atoms() {
        let (_dir, mut v) = vault();
        v.activate_model("model").unwrap();
        insert_atom(&v, "b", "2026-01-02T00:00:00Z", &axis(0), "model");
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(1), "model");
        let rebuild = |v: &Vault| {
            let ids = v.unindexed_atoms("model", 256).unwrap();
            assert_eq!(
                ids.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
                vec!["a", "b"]
            );
            for (atom, _, _) in ids {
                let vector = if atom == "a" { axis(1) } else { axis(0) };
                inference::assign(&v.conn, &atom, &vector, &format!("Title {atom}"), "model")
                    .unwrap();
            }
            v.conn.prepare("SELECT m.atom,m.topic,t.label FROM atom_topics m JOIN topics t ON t.id=m.topic ORDER BY m.atom,m.topic").unwrap()
                .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap()
                .collect::<std::result::Result<Vec<_>,_>>().unwrap()
        };
        let first = rebuild(&v);
        v.conn.execute("DELETE FROM topics", []).unwrap();
        assert_eq!(first, rebuild(&v));
    }

    #[test]
    fn reassignment_does_not_duplicate_old_and_new_topic_membership() {
        let (_dir, mut v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "old");
        v.activate_model("old").unwrap();
        inference::assign(&v.conn, "a", &axis(0), "Old", "old").unwrap();
        v.activate_model("new").unwrap();
        v.conn
            .execute(
                "INSERT INTO vectors VALUES(?,?,?)",
                params![
                    "a",
                    "new",
                    axis(1)
                        .iter()
                        .flat_map(|x| x.to_le_bytes())
                        .collect::<Vec<_>>()
                ],
            )
            .unwrap();
        inference::assign(&v.conn, "a", &axis(1), "New", "new").unwrap();
        inference::assign(&v.conn, "a", &axis(1), "New", "new").unwrap();
        let count: i64 = v
            .conn
            .query_row("SELECT count(*) FROM atom_topics WHERE atom='a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        let wrong:i64=v.conn.query_row("SELECT count(*) FROM atom_topics m JOIN topics t ON t.id=m.topic WHERE t.model<>'new'",[],|r|r.get(0)).unwrap();
        assert_eq!(wrong, 0);
    }

    #[test]
    fn deleting_one_atom_preserves_unrelated_topics() {
        let (_dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "model");
        insert_atom(&v, "b", "2026-01-02T00:00:00Z", &axis(1), "model");
        inference::assign(&v.conn, "a", &axis(0), "A", "model").unwrap();
        inference::assign(&v.conn, "b", &axis(1), "B", "model").unwrap();
        v.conn
            .execute("DELETE FROM atoms WHERE id='a'", [])
            .unwrap();
        inference::activity(&v.conn, Some("model")).unwrap();
        let labels: Vec<String> = v
            .conn
            .prepare("SELECT label FROM topics ORDER BY label")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(labels, vec!["Title b"]);
    }

    #[test]
    fn v1_trigger_migrates_without_erasing_other_topics() {
        let (dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "model");
        insert_atom(&v, "b", "2026-01-02T00:00:00Z", &axis(1), "model");
        inference::assign(&v.conn, "a", &axis(0), "A", "model").unwrap();
        inference::assign(&v.conn, "b", &axis(1), "B", "model").unwrap();
        downgrade_feedback_to_schema3(&v);
        v.conn
            .execute_batch(
                "DROP TRIGGER topic_invalidate;
CREATE TRIGGER topic_invalidate AFTER DELETE ON atoms BEGIN DELETE FROM topics; END;
PRAGMA user_version=1;",
            )
            .unwrap();
        drop(v);
        let upgraded = Vault::open(&dir.path().join("vault.sqlite")).unwrap();
        let version: i64 = upgraded
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);
        upgraded
            .conn
            .execute("DELETE FROM atoms WHERE id='a'", [])
            .unwrap();
        inference::activity(&upgraded.conn, Some("model")).unwrap();
        let labels: Vec<String> = upgraded
            .conn
            .prepare("SELECT label FROM topics ORDER BY label")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(labels, vec!["Title b"]);
    }

    #[test]
    fn v2_vault_adds_topic_skip_tracking_without_losing_atoms() {
        let (dir, v) = vault();
        insert_atom(&v, "a", "2026-01-01T00:00:00Z", &axis(0), "model");
        downgrade_feedback_to_schema3(&v);
        v.conn.execute_batch("DROP TRIGGER topic_deleted_reconsider_skips; DROP TABLE topic_skips; PRAGMA user_version=2;").unwrap();
        drop(v);
        let upgraded = Vault::open(&dir.path().join("vault.sqlite")).unwrap();
        let version: i64 = upgraded
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let atoms: i64 = upgraded
            .conn
            .query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);
        assert_eq!(atoms, 1);
        upgraded
            .conn
            .execute("INSERT INTO topic_skips VALUES('a','model')", [])
            .unwrap();
    }

    #[test]
    fn schema_five_rejects_misbound_cache_triggers_without_mutating_user_rows() {
        let replacements = [
            "CREATE TRIGGER topic_cache_feedback_update AFTER UPDATE ON feedback BEGIN SELECT 1; END;",
            "CREATE TRIGGER topic_cache_feedback_update AFTER UPDATE ON atoms BEGIN SELECT 1; END;",
        ];
        for replacement in replacements {
            let (_dir, v) = vault();
            insert_atom(
                &v,
                "trigger-user-row",
                "2026-10-01T00:00:00Z",
                &axis(0),
                "model",
            );
            let path = v.path.clone();
            v.conn
                .execute_batch(&format!(
                    "DROP TRIGGER topic_cache_feedback_update; {replacement}"
                ))
                .unwrap();
            drop(v);

            assert!(
                Vault::open(&path).is_err(),
                "schema verification accepted trigger: {replacement}"
            );
            let unchanged = Connection::open(&path).unwrap();
            assert_eq!(
                unchanged
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                5
            );
            for table in ["atoms", "atom_days", "vectors"] {
                let count: i64 = unchanged
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(count, 1, "{table} changed after rejected schema open");
            }
        }
    }

    #[test]
    fn topic_label_follows_current_representative() {
        let (_dir, v) = vault();
        let first = axis(0);
        let mut later = vec![0.0; 256];
        later[0] = 0.8;
        later[1] = 0.6;
        for atom in ["a", "b", "c"] {
            let vector = if atom == "a" { &first } else { &later };
            insert_atom(&v, atom, "2026-01-01T00:00:00Z", vector, "model");
            inference::assign(&v.conn, atom, vector, &format!("Title {atom}"), "model").unwrap();
        }
        inference::activity(&v.conn, Some("model")).unwrap();
        let label: String = v
            .conn
            .query_row("SELECT label FROM topics", [], |r| r.get(0))
            .unwrap();
        assert_eq!(label, "Title b");
    }

    #[test]
    fn topic_capacity_skip_does_not_leave_permanent_pending_work() {
        let (_dir, v) = vault();
        for index in 0..=algorithm::MAX_TOPICS {
            let atom = format!("atom-{index:03}");
            let vector = axis(index);
            insert_atom(&v, &atom, "2026-01-01T00:00:00Z", &vector, "model");
            inference::assign(&v.conn, &atom, &vector, &atom, "model").unwrap();
        }
        assert_eq!(v.pending_atoms_for_model(Some("model")).unwrap(), 0);
        let skipped: i64 = v
            .conn
            .query_row("SELECT count(*) FROM topic_skips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(skipped, 1);
        v.conn
            .execute("DELETE FROM atoms WHERE id='atom-000'", [])
            .unwrap();
        assert_eq!(v.pending_atoms_for_model(Some("model")).unwrap(), 1);
    }

    fn insert_pending_atom(v: &Vault, atom: &str) {
        insert_atom(v, atom, "2026-01-01T00:00:00Z", &axis(0), "model");
        v.conn
            .execute("DELETE FROM vectors WHERE atom=?", [atom])
            .unwrap();
    }

    fn vector_blob(v: &Vault, atom: &str) -> Vec<u8> {
        v.conn
            .query_row("SELECT vector FROM vectors WHERE atom=?", [atom], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn topic_only_refresh_skips_encoder_and_preserves_observation_vector() {
        let (_dir, mut v) = vault();
        let atom = "topic-only";
        insert_atom(&v, atom, "2026-01-01T00:00:00Z", &axis(0), "model");
        let before = vector_blob(&v, atom);
        v.activate_model("model").unwrap();
        let mut encode_calls = 0;

        let result = v
            .refresh_with_encode_fn("model", 60_000, |_| {
                encode_calls += 1;
                Some(axis(1))
            })
            .unwrap();

        assert_eq!(encode_calls, 0);
        assert_eq!(result["processed"], 1);
        assert_eq!(result["pending_atoms"], 0);
        assert_eq!(vector_blob(&v, atom), before);
    }

    #[test]
    fn incremental_refresh_updates_cache_without_reference_or_label_scans() {
        const BATCH: usize = 32;
        let (_dir, mut v) = vault();
        v.set_policy(
            "source",
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();

        let make_batch = |start: usize| {
            (start..start + BATCH)
                .map(|index| Event {
                    event_id: id(),
                    visit_id: id(),
                    site_key: "example.com".to_owned(),
                    site_epoch: 0,
                    observed_at: now(),
                    kind: "visit".to_owned(),
                    title: format!("Desk lamp installation {index:03}"),
                    search_query: None,
                    foreground_seconds: 30,
                })
                .collect::<Vec<_>>()
        };
        for start in [0, BATCH] {
            let response = v.ingest("source", 1, make_batch(start)).unwrap();
            assert_eq!(
                response["acknowledged_ids"].as_array().unwrap().len(),
                BATCH
            );
            let mut encode_calls = 0;
            if start == BATCH {
                inference::reset_work_counters();
            }
            let result = v
                .refresh_with_encode_fn("model", 60_000, |_| {
                    encode_calls += 1;
                    Some(axis(0))
                })
                .unwrap();
            assert_eq!(encode_calls, BATCH);
            assert_eq!(result["processed"], BATCH);
            assert_eq!(result["pending_atoms"], 0);
            if start == 0 {
                let topic_count: i64 = v
                    .conn
                    .query_row("SELECT count(*) FROM topics", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(topic_count, 1);
            }
        }

        let (reference_rows, delta_rows, label_rows) = inference::work_counters();
        assert_eq!(reference_rows, 0, "incremental refresh rescanned members");
        assert_eq!(delta_rows, BATCH as u64);
        assert_eq!(label_rows, 0, "incremental refresh scanned representatives");
        let cache_valid: i64 = v
            .conn
            .query_row("SELECT valid FROM topic_cache", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cache_valid, 1);
    }

    #[test]
    fn refresh_aborts_encoded_work_after_privacy_generation_changes() {
        let (_dir, mut v) = vault();
        let atom = id();
        insert_pending_atom(&v, &atom);
        let mut other = Vault::open(&v.path).unwrap();
        let mut encode_calls = 0;

        let result = v
            .refresh_with_encode_fn("model", 60_000, |_| {
                encode_calls += 1;
                other.feedback("source", &atom, "do_not_use", None).unwrap();
                Some(axis(1))
            })
            .unwrap();

        assert_eq!(encode_calls, 1);
        assert_eq!(result["processed"], 0);
        assert_eq!(result["pending_atoms"], 1);
        let derived_rows: i64 = v
            .conn
            .query_row(
                "SELECT (SELECT count(*) FROM vectors WHERE atom=?1)
                      + (SELECT count(*) FROM atom_topics WHERE atom=?1)
                      + (SELECT count(*) FROM topic_skips WHERE atom=?1)",
                [&atom],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derived_rows, 0);
    }

    #[test]
    fn refresh_aborts_encoded_work_after_active_model_changes() {
        let (_dir, mut v) = vault();
        let atom = "model-race";
        insert_pending_atom(&v, atom);
        let mut other = Vault::open(&v.path).unwrap();
        let mut encode_calls = 0;

        let result = v
            .refresh_with_encode_fn("model", 60_000, |_| {
                encode_calls += 1;
                other.activate_model("other-model").unwrap();
                Some(axis(1))
            })
            .unwrap();

        assert_eq!(encode_calls, 1);
        assert_eq!(result["processed"], 0);
        assert_eq!(result["pending_atoms"], 1);
        let derived_rows: i64 = v
            .conn
            .query_row(
                "SELECT (SELECT count(*) FROM vectors WHERE atom=?1)
                      + (SELECT count(*) FROM atom_topics WHERE atom=?1)
                      + (SELECT count(*) FROM topic_skips WHERE atom=?1)",
                [atom],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derived_rows, 0);
    }

    #[test]
    fn refresh_cannot_recreate_work_for_an_atom_deleted_during_encoding() {
        let (_dir, mut v) = vault();
        let atom = "delete-race";
        insert_pending_atom(&v, atom);
        let other = Vault::open(&v.path).unwrap();
        let mut encode_calls = 0;

        let result = v
            .refresh_with_encode_fn("model", 60_000, |_| {
                encode_calls += 1;
                other
                    .conn
                    .execute("DELETE FROM atoms WHERE id=?", [atom])
                    .unwrap();
                Some(axis(1))
            })
            .unwrap();

        assert_eq!(encode_calls, 1);
        assert_eq!(result["processed"], 0);
        assert_eq!(result["pending_atoms"], 0);
        let atom_rows: i64 = v
            .conn
            .query_row("SELECT count(*) FROM atoms WHERE id=?", [atom], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(atom_rows, 0);
        let derived_rows: i64 = v
            .conn
            .query_row(
                "SELECT (SELECT count(*) FROM vectors WHERE atom=?1)
                      + (SELECT count(*) FROM atom_topics WHERE atom=?1)
                      + (SELECT count(*) FROM topic_skips WHERE atom=?1)",
                [atom],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derived_rows, 0);
    }

    #[test]
    fn deleting_topic_member_reuses_survivor_vector_during_reconsideration() {
        let (_dir, mut v) = vault();
        let removed = "removed-member";
        let survivor = "surviving-member";
        insert_atom(&v, removed, "2026-01-01T00:00:00Z", &axis(0), "model");
        insert_atom(&v, survivor, "2026-01-02T00:00:00Z", &axis(0), "model");
        v.activate_model("model").unwrap();
        inference::assign(&v.conn, removed, &axis(0), "Removed", "model").unwrap();
        inference::assign(&v.conn, survivor, &axis(0), "Survivor", "model").unwrap();
        let before = vector_blob(&v, survivor);

        v.conn
            .execute("DELETE FROM atoms WHERE id=?", [removed])
            .unwrap();
        assert_eq!(v.pending_atoms_for_model(Some("model")).unwrap(), 1);
        let mut encode_calls = 0;
        let result = v
            .refresh_with_encode_fn("model", 60_000, |_| {
                encode_calls += 1;
                Some(axis(1))
            })
            .unwrap();

        assert_eq!(encode_calls, 0);
        assert_eq!(result["processed"], 1);
        assert_eq!(result["pending_atoms"], 0);
        assert_eq!(vector_blob(&v, survivor), before);
    }
}
