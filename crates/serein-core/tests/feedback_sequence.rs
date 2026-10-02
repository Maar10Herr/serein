use rusqlite::{params, types::Value as SqlValue, Connection};
use serde_json::{json, Value};
use serein_core::{id, model, now, storage::Vault, Event, Policy, Recall};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

const FIXED_TIME: &str = "2026-10-01T12:34:56Z";
const FIRST_CONFIRMATION_ID: &str = "ffffffff-ffff-4fff-bfff-ffffffffffff";
const SECOND_CONFIRMATION_ID: &str = "00000000-0000-4000-8000-000000000001";
const SOURCE_ID: &str = "11111111-1111-4111-8111-111111111111";
const TOPIC_ID: &str = "33333333-3333-4333-8333-333333333333";

static DATA_DIR_LOCK: Mutex<()> = Mutex::new(());

struct DataDirGuard(Option<OsString>);

impl DataDirGuard {
    fn set(path: &Path) -> Self {
        let old = std::env::var_os("SEREIN_DATA_DIR");
        std::env::set_var("SEREIN_DATA_DIR", path);
        Self(old)
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            std::env::set_var("SEREIN_DATA_DIR", old);
        } else {
            std::env::remove_var("SEREIN_DATA_DIR");
        }
    }
}

fn recall_request(query: &str, scope: &[&str]) -> Recall {
    Recall {
        protocol: 1,
        request_id: id(),
        client: "feedback-sequence-test".into(),
        vault: "default".into(),
        query: query.into(),
        facets: vec![],
        scope: scope.iter().map(|value| (*value).to_owned()).collect(),
        max_bytes: 4096,
        budget_ms: 1500,
    }
}

fn make_event(title: &str, query: Option<&str>) -> Event {
    Event {
        event_id: id(),
        visit_id: id(),
        site_key: "research.example.org".into(),
        site_epoch: 0,
        observed_at: now(),
        kind: if query.is_some() { "search" } else { "visit" }.into(),
        title: title.into(),
        search_query: query.map(str::to_owned),
        foreground_seconds: 20,
    }
}

fn set_latest_feedback_identity(vault: &Vault, id: &str) {
    let seq: i64 = vault
        .conn
        .query_row("SELECT max(seq) FROM feedback", [], |row| row.get(0))
        .unwrap();
    let changed = vault
        .conn
        .execute(
            "UPDATE feedback SET id=?1,time=?2 WHERE seq=?3",
            params![id, FIXED_TIME, seq],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

fn add_fixed_confirmation(vault: &mut Vault, atom: &str, text: &str, id: &str) {
    vault
        .feedback(SOURCE_ID, atom, "confirm_constraint", Some(text))
        .unwrap();
    set_latest_feedback_identity(vault, id);
}

fn assert_latest_confirmation_is_consistent(vault: &mut Vault, atom: &str, db_root: &Path) {
    let dashboard = vault.dashboard(SOURCE_ID).unwrap();
    let card = dashboard["cards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["id"] == atom)
        .expect("confirmed observation remains on its dashboard card");
    assert_eq!(card["state"], "confirmed");
    assert_eq!(
        card["text"],
        "Telescope primary mirror supports a forty six millimeter objective."
    );

    let latest = vault
        .recall(
            &recall_request("telescope objective", &["confirmed_preferences"]),
            SOURCE_ID,
            &db_root.join("missing-model"),
        )
        .unwrap();
    let context = latest["context"].as_array().unwrap();
    let record = context
        .iter()
        .find(|record| record["id"] == atom)
        .expect("latest confirmation is a recall candidate");
    assert_eq!(
        record["text"],
        "Telescope primary mirror supports a forty six millimeter objective."
    );

    let superseded = vault
        .recall(
            &recall_request("aperture", &["confirmed_preferences"]),
            SOURCE_ID,
            &db_root.join("missing-model"),
        )
        .unwrap();
    assert!(
        superseded["context"].as_array().unwrap().is_empty(),
        "superseded confirmation text must not remain a candidate"
    );

    let explanation = vault.explain(SOURCE_ID, &[atom.to_owned()], 4096).unwrap();
    let corrections = explanation["evidence"][0]["corrections"]
        .as_array()
        .unwrap();
    let confirmation_texts: Vec<_> = corrections
        .iter()
        .filter(|correction| correction["action"] == "confirm_constraint")
        .map(|correction| correction["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        confirmation_texts,
        vec![
            "Telescope primary mirror supports a forty two millimeter aperture.",
            "Telescope primary mirror supports a forty six millimeter objective."
        ]
    );
}

fn ordered_triplets(value: &Value, collection: &str) -> Vec<Value> {
    value[collection]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| json!({"id":record["id"],"text":record["text"],"state":record["state"]}))
        .collect()
}

#[test]
fn p01_secondary_indexes_preserve_ordered_public_ids_text_and_states() {
    let _lock = DATA_DIR_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let model_dir = temp.path().join("models/current");
    fs::create_dir_all(&model_dir).unwrap();
    fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/pack/manifest.json"
        ),
        model_dir.join("manifest.json"),
    )
    .unwrap();
    let model_hash = model::current_hash(&model_dir).unwrap();

    let db_path = temp.path().join("index-output.sqlite");
    let mut vault = Vault::open(&db_path).unwrap();
    vault
        .set_policy(
            SOURCE_ID,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    let events: Vec<_> = (0..24)
        .map(|index| Event {
            event_id: id(),
            visit_id: id(),
            site_key: format!("chairs-{}.example.org", index % 8),
            site_epoch: 0,
            observed_at: now(),
            kind: "search".into(),
            title: format!("Ergonomic office chair installation guide {index}"),
            search_query: Some(format!("ergonomic office chair installation guide {index}")),
            foreground_seconds: 20,
        })
        .collect();
    vault.ingest(SOURCE_ID, 1, events).unwrap();
    let atom_ids: Vec<String> = vault
        .conn
        .prepare("SELECT id FROM atoms WHERE source=?1 ORDER BY id")
        .unwrap()
        .query_map([SOURCE_ID], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(atom_ids.len(), 24);

    let topic = id();
    vault
        .conn
        .execute(
            "INSERT INTO topics(id,label,centroid,model) VALUES(?1,?2,?3,?4)",
            params![
                topic,
                "Ergonomic office chairs",
                vec![0u8; 256 * 4],
                model_hash
            ],
        )
        .unwrap();
    for atom in &atom_ids {
        vault
            .conn
            .execute(
                "INSERT INTO atom_topics(atom,topic,mass) VALUES(?1,?2,1.0)",
                params![atom, topic],
            )
            .unwrap();
    }
    for atom in atom_ids.iter().take(3) {
        vault
            .feedback(
                SOURCE_ID,
                atom,
                "confirm_constraint",
                Some("Ergonomic office chair provides adjustable lumbar support."),
            )
            .unwrap();
    }
    for atom in atom_ids.iter().skip(3).take(2) {
        vault.feedback(SOURCE_ID, atom, "do_not_use", None).unwrap();
    }

    let request = recall_request(
        "ergonomic office chair",
        &["research", "confirmed_preferences"],
    );
    let before_dashboard = vault.dashboard(SOURCE_ID).unwrap();
    let before_recall = vault
        .recall(&request, SOURCE_ID, &temp.path().join("missing-model"))
        .unwrap();
    let explanation_ids: Vec<_> = before_recall["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_owned())
        .collect();
    let before_explain = vault.explain(SOURCE_ID, &explanation_ids, 4096).unwrap();
    let before_dashboard_rows = ordered_triplets(&before_dashboard, "cards");
    let before_recall_rows = ordered_triplets(&before_recall, "context");
    let before_explain_ids: Vec<_> = before_explain["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].clone())
        .collect();

    vault
        .conn
        .execute_batch(
            "DROP INDEX atoms_by_source_recency;
             DROP INDEX memberships_by_topic;
             DROP INDEX feedback_by_atom_action;",
        )
        .unwrap();
    let after_dashboard = vault.dashboard(SOURCE_ID).unwrap();
    let after_recall = vault
        .recall(&request, SOURCE_ID, &temp.path().join("missing-model"))
        .unwrap();
    let after_explain = vault.explain(SOURCE_ID, &explanation_ids, 4096).unwrap();

    assert_eq!(
        ordered_triplets(&after_dashboard, "cards"),
        before_dashboard_rows,
        "secondary indexes changed dashboard order, text, or state"
    );
    assert_eq!(
        ordered_triplets(&after_recall, "context"),
        before_recall_rows,
        "secondary indexes changed recall order, text, or state"
    );
    assert_eq!(
        after_explain["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["id"].clone())
            .collect::<Vec<_>>(),
        before_explain_ids,
        "secondary indexes changed explanation evidence IDs"
    );
    assert!(before_recall_rows
        .iter()
        .any(|record| record["state"] == "confirmed"));
    assert!(before_recall_rows
        .iter()
        .any(|record| record["state"] == "observed"));
    assert!(before_recall_rows.len() <= 6);
}

#[test]
fn r01_same_second_feedback_uses_write_sequence_for_every_consumer_and_after_reopen() {
    let _lock = DATA_DIR_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let db_path = temp.path().join("feedback.sqlite");
    let mut vault = Vault::open(&db_path).unwrap();
    vault
        .set_policy(
            SOURCE_ID,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    vault
        .ingest(
            SOURCE_ID,
            1,
            vec![make_event("Telescope mirror reference", None)],
        )
        .unwrap();
    let atom: String = vault
        .conn
        .query_row("SELECT id FROM atoms WHERE source=?1", [SOURCE_ID], |row| {
            row.get(0)
        })
        .unwrap();
    add_fixed_confirmation(
        &mut vault,
        &atom,
        "Telescope primary mirror supports a forty two millimeter aperture.",
        FIRST_CONFIRMATION_ID,
    );
    add_fixed_confirmation(
        &mut vault,
        &atom,
        "Telescope primary mirror supports a forty six millimeter objective.",
        SECOND_CONFIRMATION_ID,
    );

    assert_latest_confirmation_is_consistent(&mut vault, &atom, temp.path());
    drop(vault);

    let mut reopened = Vault::open(&db_path).unwrap();
    assert_latest_confirmation_is_consistent(&mut reopened, &atom, temp.path());

    let confirmation_rows: Vec<(String, String, i64)> = reopened
        .conn
        .prepare("SELECT id,text,seq FROM feedback WHERE action='confirm_constraint' ORDER BY seq")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(confirmation_rows[0].0, FIRST_CONFIRMATION_ID);
    assert_eq!(confirmation_rows[1].0, SECOND_CONFIRMATION_ID);
    assert!(confirmation_rows[0].2 < confirmation_rows[1].2);
    assert_eq!(
        confirmation_rows[0].1,
        "Telescope primary mirror supports a forty two millimeter aperture."
    );
    assert_eq!(
        confirmation_rows[1].1,
        "Telescope primary mirror supports a forty six millimeter objective."
    );
}

#[test]
fn r03_later_confirmation_does_not_resurrect_suppressed_record_and_ids_stay_uuids() {
    let _lock = DATA_DIR_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let mut vault = Vault::open(&temp.path().join("suppression.sqlite")).unwrap();
    vault
        .set_policy(
            SOURCE_ID,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    let events = [
        make_event(
            "Desk lamp wall mount setup guide",
            Some("desk lamp wall mount anchor setup"),
        ),
        make_event(
            "Ceiling lamp joist bracket installation guide",
            Some("ceiling lamp joist bracket installation"),
        ),
    ];
    vault.ingest(SOURCE_ID, 1, events.to_vec()).unwrap();
    let atoms: Vec<String> = vault
        .conn
        .prepare("SELECT id FROM atoms WHERE source=?1 ORDER BY query")
        .unwrap()
        .query_map([SOURCE_ID], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(atoms.len(), 2);
    for (atom, action, text) in [
        (
            atoms[0].as_str(),
            "wrong_topic",
            "Ceiling lamp joist bracket requires a secure beam.",
        ),
        (
            atoms[1].as_str(),
            "do_not_use",
            "Desk lamp wall mount needs rated anchor bolts.",
        ),
    ] {
        vault.feedback(SOURCE_ID, atom, action, None).unwrap();
        vault
            .feedback(SOURCE_ID, atom, "confirm_constraint", Some(text))
            .unwrap();
    }

    let ids: Vec<String> = vault
        .conn
        .prepare("SELECT id FROM feedback ORDER BY seq")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(ids.len(), 4);
    assert!(ids.iter().all(|value| uuid::Uuid::parse_str(value).is_ok()));

    for query in ["ceiling lamp joist bracket", "desk lamp wall mount anchor"] {
        let response = vault
            .recall(
                &recall_request(query, &["research", "confirmed_preferences"]),
                SOURCE_ID,
                &temp.path().join("missing-model"),
            )
            .unwrap();
        assert!(
            response["context"].as_array().unwrap().is_empty(),
            "a later confirmation cannot undo retained suppression for {query}"
        );
    }

    let explanation = vault.explain(SOURCE_ID, &atoms, 4096).unwrap();
    assert!(
        explanation["evidence"].as_array().unwrap().is_empty(),
        "explanation uses the shared do_not_use/wrong_topic suppression result"
    );

    let dashboard = vault.dashboard(SOURCE_ID).unwrap();
    let cards = dashboard["cards"].as_array().unwrap();
    for (atom, action) in atoms.iter().zip(["wrong_topic", "do_not_use"]) {
        let card = cards
            .iter()
            .find(|card| card["id"].as_str() == Some(atom.as_str()))
            .expect("owner-facing dashboard keeps suppressed observation cards");
        assert_eq!(card["state"], "confirmed");
        assert_eq!(card["prominent"], false);
        assert!(card["corrections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|correction| { correction["action"] == action }));
    }
}

fn create_v3_fixture(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
PRAGMA journal_mode=DELETE;
CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
INSERT INTO meta VALUES('evidence','17'),('privacy','23');
CREATE TABLE sources(id TEXT PRIMARY KEY,policy TEXT NOT NULL);
CREATE TABLE site_rules(source TEXT NOT NULL,site TEXT NOT NULL,epoch INTEGER NOT NULL,excluded INTEGER NOT NULL,PRIMARY KEY(source,site));
CREATE TABLE atoms(id TEXT PRIMARY KEY,source TEXT NOT NULL,site TEXT NOT NULL,title TEXT NOT NULL,query TEXT,kind TEXT NOT NULL,first_seen TEXT NOT NULL,last_seen TEXT NOT NULL,seconds INTEGER NOT NULL,canonical TEXT NOT NULL UNIQUE);
CREATE TABLE events(source TEXT NOT NULL,id TEXT NOT NULL,atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,visit TEXT NOT NULL,time TEXT NOT NULL,PRIMARY KEY(source,id));
CREATE TABLE atom_days(atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,day TEXT NOT NULL,session TEXT NOT NULL,mass REAL NOT NULL,PRIMARY KEY(atom,day,session));
CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query,tokenize='unicode61');
CREATE TRIGGER atom_insert AFTER INSERT ON atoms BEGIN INSERT INTO atom_fts(id,title,query) VALUES(new.id,new.title,new.query); END;
CREATE TRIGGER atom_delete AFTER DELETE ON atoms BEGIN DELETE FROM atom_fts WHERE id=old.id; END;
CREATE TABLE vectors(atom TEXT PRIMARY KEY REFERENCES atoms(id) ON DELETE CASCADE,model TEXT NOT NULL,vector BLOB NOT NULL);
CREATE TABLE feedback(id TEXT PRIMARY KEY,atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,action TEXT NOT NULL,text TEXT,time TEXT NOT NULL);
CREATE TABLE receipts(id TEXT PRIMARY KEY,ids TEXT NOT NULL,evidence INTEGER NOT NULL,privacy INTEGER NOT NULL,bytes INTEGER NOT NULL,time TEXT NOT NULL,status TEXT NOT NULL);
CREATE TABLE topics(id TEXT PRIMARY KEY,label TEXT NOT NULL,centroid BLOB NOT NULL,model TEXT NOT NULL);
CREATE TABLE atom_topics(atom TEXT NOT NULL REFERENCES atoms(id) ON DELETE CASCADE,topic TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,mass REAL NOT NULL,PRIMARY KEY(atom,topic));
CREATE TABLE topic_skips(atom TEXT PRIMARY KEY REFERENCES atoms(id) ON DELETE CASCADE,model TEXT NOT NULL);
CREATE TRIGGER topic_deleted_reconsider_skips AFTER DELETE ON topics BEGIN DELETE FROM topic_skips WHERE model=old.model; END;
CREATE TRIGGER topic_invalidate BEFORE DELETE ON atoms BEGIN DELETE FROM topics WHERE id IN (SELECT topic FROM atom_topics WHERE atom=old.id); END;
PRAGMA user_version=3;",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO sources(id,policy) VALUES(?1,?2)",
        params![
            SOURCE_ID,
            serde_json::to_string(&Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 4,
                ..Policy::default()
            })
            .unwrap()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO site_rules VALUES(?1,'research.example.org',7,0)",
        [SOURCE_ID],
    )
    .unwrap();

    let atoms = [
        (
            "22222222-2222-4222-8222-222222222222",
            "research.example.org",
            "Telescope mirror guide",
            Some("telescope mirror alignment"),
            "search",
            "2026-09-01T10:00:00Z",
            "2026-09-02T10:00:00Z",
            42_i64,
            "canonical-telescope",
        ),
        (
            "44444444-4444-4444-8444-444444444444",
            "desk.example.org",
            "Desk lamp installation",
            None,
            "visit",
            "2026-09-03T10:00:00Z",
            "2026-09-04T10:00:00Z",
            18_i64,
            "canonical-desk",
        ),
        (
            "55555555-5555-4555-8555-555555555555",
            "chair.example.org",
            "Office chair guide",
            Some("office chair ergonomics"),
            "search",
            "2026-09-05T10:00:00Z",
            "2026-09-06T10:00:00Z",
            31_i64,
            "canonical-chair",
        ),
    ];
    for (atom, site, title, query, kind, first_seen, last_seen, seconds, canonical) in atoms {
        conn.execute(
            "INSERT INTO atoms VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                atom, SOURCE_ID, site, title, query, kind, first_seen, last_seen, seconds,
                canonical
            ],
        )
        .unwrap();
    }
    for (offset, atom) in [
        "22222222-2222-4222-8222-222222222222",
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
    ]
    .into_iter()
    .enumerate()
    {
        conn.execute(
            "INSERT INTO events VALUES(?1,?2,?3,?4,?5)",
            params![
                SOURCE_ID,
                format!("66666666-6666-4666-8666-{offset:012x}"),
                atom,
                format!("77777777-7777-4777-8777-{offset:012x}"),
                FIXED_TIME
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO atom_days VALUES(?1,'2026-09-01',?2,?3)",
            params![atom, format!("window-{offset}"), 1.0 + offset as f64],
        )
        .unwrap();
        let vector: Vec<u8> = (0..256)
            .map(|index| ((index + offset) % 251) as u8)
            .collect();
        conn.execute(
            "INSERT INTO vectors VALUES(?1,'model-sha256-test',?2)",
            params![atom, vector],
        )
        .unwrap();
    }

    let centroid: Vec<u8> = (0..256).map(|index| (index % 251) as u8).collect();
    conn.execute(
        "INSERT INTO topics VALUES(?1,'Telescope and lamps',?2,'model-sha256-test')",
        params![TOPIC_ID, centroid],
    )
    .unwrap();
    for atom in [
        "22222222-2222-4222-8222-222222222222",
        "44444444-4444-4444-8444-444444444444",
    ] {
        conn.execute(
            "INSERT INTO atom_topics VALUES(?1,?2,0.75)",
            params![atom, TOPIC_ID],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO topic_skips VALUES('55555555-5555-4555-8555-555555555555','model-sha256-test')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO receipts VALUES('88888888-8888-4888-8888-888888888888','[\"22222222-2222-4222-8222-222222222222\"]',17,23,384,'2026-09-30T11:00:00Z','ok')",
        [],
    )
    .unwrap();

    for (feedback_id, atom, action, text) in [
        (
            "ffffffff-ffff-4fff-bfff-fffffffffff1",
            "22222222-2222-4222-8222-222222222222",
            "do_not_use",
            None,
        ),
        (
            "00000000-0000-4000-8000-000000000001",
            "22222222-2222-4222-8222-222222222222",
            "confirm_constraint",
            Some("Telescope primary mirror aperture is 42 millimeters."),
        ),
        (
            "eeeeeeee-eeee-4eee-beee-eeeeeeeeeee2",
            "22222222-2222-4222-8222-222222222222",
            "wrong_topic",
            None,
        ),
        (
            "11111111-1111-4111-8111-111111111112",
            "22222222-2222-4222-8222-222222222222",
            "confirm_constraint",
            Some("Telescope primary mirror aperture is 46 millimeters."),
        ),
        (
            "dddddddd-dddd-4ddd-bddd-ddddddddddd3",
            "44444444-4444-4444-8444-444444444444",
            "temporary_research",
            None,
        ),
        (
            "cccccccc-cccc-4ccc-bccc-ccccccccccc4",
            "55555555-5555-4555-8555-555555555555",
            "not_about_me",
            None,
        ),
    ] {
        conn.execute(
            "INSERT INTO feedback(id,atom,action,text,time) VALUES(?1,?2,?3,?4,?5)",
            params![feedback_id, atom, action, text, FIXED_TIME],
        )
        .unwrap();
    }
    assert!(foreign_key_violations(&conn).is_empty());
    conn
}

fn table_snapshot(conn: &Connection, table: &str) -> Vec<Vec<SqlValue>> {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, SqlValue>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn feedback_snapshot_v3(
    conn: &Connection,
) -> Vec<(i64, String, String, String, Option<String>, String)> {
    conn.prepare("SELECT rowid,id,atom,action,text,time FROM feedback ORDER BY rowid")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn foreign_key_violations(conn: &Connection) -> Vec<(String, i64, String, i64)> {
    conn.prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn atom_fts_trigger_snapshot(conn: &Connection) -> Vec<(String, String, String)> {
    conn.prepare(
        "SELECT name,tbl_name,sql FROM sqlite_master
         WHERE type='trigger' AND name IN ('atom_insert','atom_delete') ORDER BY name",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

fn schema_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
        != 0
}

fn assert_required_index_columns(conn: &Connection, name: &str, expected: &[(&str, bool)]) {
    let mut statement = conn
        .prepare(&format!("PRAGMA index_xinfo('{name}')"))
        .unwrap();
    let actual: Vec<(String, bool)> = statement
        .query_map([], |row| {
            let is_key: bool = row.get(5)?;
            let column: Option<String> = row.get(2)?;
            let descending: bool = row.get(3)?;
            Ok((is_key, column, descending))
        })
        .unwrap()
        .filter_map(|row| {
            let (is_key, column, descending) = row.unwrap();
            if is_key {
                Some((
                    column.expect("every indexed key column has a name"),
                    descending,
                ))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        actual,
        expected
            .iter()
            .map(|(column, descending)| ((*column).to_owned(), *descending))
            .collect::<Vec<_>>(),
        "unexpected columns or sort directions in {name}"
    );
}

fn old_reader_cli_paths() -> [(PathBuf, &'static str); 2] {
    [
        (
            PathBuf::from(
                std::env::var_os("SEREIN_BASELINE_BINARY")
                    .expect("set SEREIN_BASELINE_BINARY to the pinned schema-3 CLI for R02"),
            ),
            "schema3",
        ),
        (
            PathBuf::from(
                std::env::var_os("SEREIN_SCHEMA4_BINARY")
                    .expect("set SEREIN_SCHEMA4_BINARY to the pinned schema-4 CLI for R02"),
            ),
            "schema4",
        ),
    ]
}

fn assert_old_readers_reject_schema5(db_path: &Path, temp: &tempfile::TempDir) {
    for (reader, version) in old_reader_cli_paths() {
        assert!(
            reader.is_file(),
            "pinned {version} CLI is missing at {}; set its configured reader binary",
            reader.display()
        );
        let isolated_root = temp.path().join(format!("old-reader-{version}"));
        let vault_id = "99999999-9999-4999-8999-999999999999";
        let vault_dir = isolated_root.join("vaults").join(vault_id);
        fs::create_dir_all(&vault_dir).unwrap();
        fs::copy(db_path, vault_dir.join("context.sqlite")).unwrap();
        fs::write(
            isolated_root.join("connections.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "default_vault": vault_id,
                "connections": [{
                    "source_id": SOURCE_ID,
                    "vault_id": vault_id,
                    "extension_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "browser": "chrome",
                    "nonce_hash": "test-only",
                    "expires_at": 4_102_444_800_i64,
                    "paired": true,
                    "adapters": [],
                    "skill_install_requested": false,
                    "skill_repository": null,
                    "label": "Disposable migration compatibility test"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let request = json!({
            "protocol": 1,
            "request_id": id(),
            "client": "migration-test",
            "vault": "default",
            "query": "telescope mirror",
            "facets": [],
            "scope": ["research"],
            "max_bytes": 4096,
            "budget_ms": 1000
        });
        let mut child = Command::new(reader)
            .args(["recall", "--request-stdin", "--json"])
            .env("SEREIN_DATA_DIR", &isolated_root)
            .env("SEREIN_INSTALL_HOME", isolated_root.join("install"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success(), "{version} binary opened schema 5");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["error"]["code"], "SCHEMA_TOO_NEW", "{version}");
    }
}

#[test]
fn r02_v3_migration_preserves_all_rows_sequences_and_rolls_back_on_failure() {
    let _lock = DATA_DIR_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let db_path = temp.path().join("legacy-v3.sqlite");
    let legacy = create_v3_fixture(&db_path);
    assert_eq!(schema_version(&legacy), 3);
    let feedback_before = feedback_snapshot_v3(&legacy);
    let atom_fts_triggers_before = atom_fts_trigger_snapshot(&legacy);
    assert_eq!(atom_fts_triggers_before.len(), 2);
    assert_eq!(feedback_before.len(), 6);
    let preserved_tables = [
        "sources",
        "site_rules",
        "atoms",
        "events",
        "atom_days",
        "atom_fts",
        "vectors",
        "receipts",
        "topics",
        "atom_topics",
        "topic_skips",
    ];
    let snapshots_before: Vec<_> = preserved_tables
        .iter()
        .map(|table| (*table, table_snapshot(&legacy, table)))
        .collect();
    assert_eq!(
        legacy
            .query_row("SELECT value FROM meta WHERE key='evidence'", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "17"
    );
    assert_eq!(foreign_key_violations(&legacy), Vec::new());
    drop(legacy);

    // The index name is valid for schema 4, but its deliberately wrong
    // definition forces the late index-validation error inside the migration.
    let conflicted = Connection::open(&db_path).unwrap();
    conflicted
        .execute_batch("CREATE INDEX atoms_by_source_recency ON atoms(id);")
        .unwrap();
    drop(conflicted);
    let failed = Vault::open(&db_path);
    assert!(
        failed.is_err(),
        "conflicting index definition must block migration"
    );

    let rolled_back = Connection::open(&db_path).unwrap();
    assert_eq!(schema_version(&rolled_back), 3);
    assert!(!table_exists(&rolled_back, "feedback_v4"));
    assert!(!rolled_back
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='feedback'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap()
        .contains("seq INTEGER"));
    assert_eq!(feedback_snapshot_v3(&rolled_back), feedback_before);
    assert_eq!(
        atom_fts_trigger_snapshot(&rolled_back),
        atom_fts_triggers_before,
        "FTS trigger definitions changed after migration rollback"
    );
    for (table, before) in &snapshots_before {
        assert_eq!(
            table_snapshot(&rolled_back, table),
            *before,
            "{table} changed after rollback"
        );
    }
    assert_eq!(foreign_key_violations(&rolled_back), Vec::new());
    rolled_back
        .execute_batch("DROP INDEX atoms_by_source_recency;")
        .unwrap();
    drop(rolled_back);

    let upgraded =
        Vault::open(&db_path).expect("valid v3 fixture migrates after removing injection");
    assert_eq!(schema_version(&upgraded.conn), 5);
    assert_eq!(foreign_key_violations(&upgraded.conn), Vec::new());
    assert_eq!(
        atom_fts_trigger_snapshot(&upgraded.conn),
        atom_fts_triggers_before,
        "feedback migration must leave atom FTS triggers unchanged"
    );
    let feedback_after: Vec<(i64, String, String, String, Option<String>, String)> = upgraded
        .conn
        .prepare("SELECT seq,id,atom,action,text,time FROM feedback ORDER BY seq")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(feedback_after.len(), feedback_before.len());
    for (new, old) in feedback_after.iter().zip(&feedback_before) {
        assert!(new.0 > 0);
        assert_eq!(
            (&new.1, &new.2, &new.3, &new.4, &new.5),
            (&old.1, &old.2, &old.3, &old.4, &old.5)
        );
    }
    assert!(feedback_after.windows(2).all(|pair| pair[0].0 < pair[1].0));
    for (table, before) in &snapshots_before {
        assert_eq!(
            table_snapshot(&upgraded.conn, table),
            *before,
            "{table} changed during migration"
        );
    }

    assert_required_index_columns(
        &upgraded.conn,
        "atoms_by_source_recency",
        &[("source", false), ("last_seen", true), ("id", false)],
    );
    assert_required_index_columns(
        &upgraded.conn,
        "memberships_by_topic",
        &[("topic", false), ("atom", false)],
    );
    assert_required_index_columns(
        &upgraded.conn,
        "feedback_by_atom_action",
        &[("atom", false), ("action", false), ("seq", false)],
    );

    upgraded
        .conn
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(upgraded);
    assert_old_readers_reject_schema5(&db_path, &temp);
}
