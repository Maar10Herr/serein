use rusqlite::params;
use serde_json::Value;
use serein_core::{id, now, storage::Vault, Policy, Recall};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

const SOURCE: &str = "11111111-1111-4111-8111-111111111111";
const STALE_TIME: &str = "2025-01-01T00:00:00Z";
const MAX_BYTES: usize = 16_384;

// Vault::feedback enforces a global 1,000-row confirmation cap. Larger
// synthetic populations such as 5,000 are unreachable through the supported
// API and are outside this regression test's storage contract.

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

fn request(query: &str) -> Recall {
    Recall {
        protocol: 1,
        request_id: id(),
        client: "confirmation-capacity-test".into(),
        vault: "default".into(),
        query: query.into(),
        facets: vec![],
        scope: vec!["confirmed_preferences".into()],
        max_bytes: MAX_BYTES,
        budget_ms: 10_000,
    }
}

fn enabled_policy() -> Policy {
    Policy {
        consent: true,
        recall_enabled: true,
        capture_epoch: 1,
        ..Policy::default()
    }
}

fn fixture_id(value: u32) -> String {
    format!("{value:08x}-0000-4000-8000-{value:012x}")
}

fn add_atom(vault: &Vault, atom: &str, title: &str, last_seen: &str) {
    vault
        .conn
        .execute(
            "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
             VALUES(?1,?2,'archive.example.org',?3,NULL,'search',?4,?4,20,?5)",
            params![atom, SOURCE, title, last_seen, format!("confirmation-capacity:{atom}")],
        )
        .unwrap();
}

// The public feedback API is used for the relevant old confirmation, its
// replacement control, the final row at capacity, and the rejected overflow.
// Bulk SQL seeds only unrelated historical rows so each boundary case can put
// the relevant record at sequence 1 without 1,000 slow setup transactions.
fn add_unrelated_confirmations(vault: &Vault, count: usize, prefix: &str) {
    let tx = vault.conn.unchecked_transaction().unwrap();
    for index in 0..count {
        let atom = fixture_id(2_000_000 + index as u32);
        let title = format!("Garden herb preference record {index}");
        tx.execute(
            "INSERT INTO atoms(
                 id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical
             ) VALUES(?1,?2,'garden.example.org',?3,NULL,'search',?4,?4,20,?5)",
            params![
                atom,
                SOURCE,
                title,
                STALE_TIME,
                format!("confirmation-capacity:{prefix}:{index}")
            ],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO feedback(id,atom,action,text,time)
             VALUES(?1,?2,'confirm_constraint',?3,?4)",
            params![
                format!("{prefix}-feedback-{index}"),
                atom,
                format!("Unrelated garden herb preference note {index}"),
                STALE_TIME
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

struct Fixture {
    vault: Vault,
    old_rx100: String,
    replaced: String,
    zv1: String,
}

fn fixture(root: &Path, confirmation_count: usize) -> Fixture {
    assert!(confirmation_count == 999 || confirmation_count == 1000);
    let path: PathBuf = root.join(format!("confirmed-{confirmation_count}.sqlite"));
    let mut vault = Vault::open(&path).unwrap();
    vault.set_policy(SOURCE, &enabled_policy()).unwrap();

    let old_rx100 = fixture_id(900_001);
    add_atom(
        &vault,
        &old_rx100,
        "Old page with unrelated data",
        STALE_TIME,
    );
    vault
        .feedback(
            SOURCE,
            &old_rx100,
            "confirm_constraint",
            Some("Sony RX100 cameras preference notes"),
        )
        .unwrap();

    let replaced = fixture_id(900_002);
    add_atom(&vault, &replaced, "Another old page", STALE_TIME);
    vault
        .feedback(
            SOURCE,
            &replaced,
            "confirm_constraint",
            Some("Sony RX100 older preference notes"),
        )
        .unwrap();
    vault
        .feedback(
            SOURCE,
            &replaced,
            "confirm_constraint",
            Some("My office desk must be oak."),
        )
        .unwrap();

    let zv1 = fixture_id(900_003);
    add_unrelated_confirmations(
        &vault,
        confirmation_count - 4,
        &format!("fill-{confirmation_count}"),
    );
    add_atom(&vault, &zv1, "A newer camera page", &now());
    vault
        .feedback(
            SOURCE,
            &zv1,
            "confirm_constraint",
            Some("Sony ZV1 cameras preference notes"),
        )
        .unwrap();

    let actual_count: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM feedback WHERE action='confirm_constraint'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actual_count, confirmation_count as i64);

    Fixture {
        vault,
        old_rx100,
        replaced,
        zv1,
    }
}

fn recall(vault: &mut Vault, query: &str, root: &Path) -> Value {
    vault
        .recall(
            &request(query),
            SOURCE,
            &root.join("missing-model-for-confirmation-capacity"),
        )
        .unwrap()
}

fn context_ids(packet: &Value) -> Vec<String> {
    packet["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_owned())
        .collect()
}

fn has_comparison_gap(packet: &Value) -> bool {
    packet["context"].as_array().unwrap().iter().any(|record| {
        record["limits"].as_array().unwrap().iter().any(|limit| {
            limit.as_str() == Some("Comparison evidence covers only one requested model.")
        })
    })
}

#[test]
fn latest_confirmations_cover_supported_capacity_without_dropping_old_relevant_evidence() {
    let _lock = DATA_DIR_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());

    for confirmation_count in [999, 1000] {
        let mut fixture = fixture(temp.path(), confirmation_count);

        let start = Instant::now();
        let ordinary = recall(
            &mut fixture.vault,
            "Sony RX100 cameras preference notes",
            temp.path(),
        );
        let ordinary_elapsed = start.elapsed();
        let ordinary_ids = context_ids(&ordinary);
        assert!(
            ordinary_ids.contains(&fixture.old_rx100),
            "the retained sequence-1 confirmation must survive {confirmation_count} total confirmations: {ordinary}"
        );
        let old_record = ordinary["context"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["id"] == fixture.old_rx100)
            .unwrap();
        assert_eq!(old_record["state"], "confirmed");
        assert_eq!(old_record["text"], "Sony RX100 cameras preference notes");

        let start = Instant::now();
        let comparison = recall(
            &mut fixture.vault,
            "Compare Sony RX100 and ZV1 cameras",
            temp.path(),
        );
        let comparison_elapsed = start.elapsed();
        let comparison_ids = context_ids(&comparison);
        assert!(
            comparison_ids.contains(&fixture.old_rx100),
            "comparison retrieval must keep the oldest RX100 confirmation at {confirmation_count}: {comparison}"
        );
        assert!(
            comparison_ids.contains(&fixture.zv1),
            "comparison retrieval must keep the ZV1 arm at {confirmation_count}: {comparison}"
        );
        assert!(!has_comparison_gap(&comparison), "{comparison}");
        assert!(comparison["context"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["state"] == "confirmed"));

        eprintln!(
            "confirmation capacity {confirmation_count}: ordinary {ordinary_elapsed:?}, comparison {comparison_elapsed:?}"
        );

        if confirmation_count == 1000 {
            // Current confirmation text wins: the superseded RX100 text does
            // not make this atom a candidate, while its current office text does.
            let replaced_old_text = recall(
                &mut fixture.vault,
                "Sony RX100 older preference notes",
                temp.path(),
            );
            assert!(!context_ids(&replaced_old_text).contains(&fixture.replaced));
            let replaced_current_text = recall(&mut fixture.vault, "office desk", temp.path());
            assert!(context_ids(&replaced_current_text).contains(&fixture.replaced));

            fixture
                .vault
                .feedback(SOURCE, &fixture.old_rx100, "wrong_topic", None)
                .unwrap();
            let suppressed = recall(
                &mut fixture.vault,
                "Sony RX100 cameras preference notes",
                temp.path(),
            );
            assert!(!context_ids(&suppressed).contains(&fixture.old_rx100));

            let overflow = fixture_id(900_004);
            add_atom(&fixture.vault, &overflow, "Overflow test page", &now());
            let rows_before: i64 = fixture
                .vault
                .conn
                .query_row("SELECT count(*) FROM feedback", [], |row| row.get(0))
                .unwrap();
            let err = fixture
                .vault
                .feedback(
                    SOURCE,
                    &overflow,
                    "confirm_constraint",
                    Some("One more explicit preference"),
                )
                .unwrap_err();
            assert_eq!(err.0, "STORAGE_FULL");
            let rows_after: i64 = fixture
                .vault
                .conn
                .query_row("SELECT count(*) FROM feedback", [], |row| row.get(0))
                .unwrap();
            assert_eq!(
                rows_after, rows_before,
                "overflow rejection must not insert a row"
            );
            let overflow_rows: i64 = fixture
                .vault
                .conn
                .query_row(
                    "SELECT count(*) FROM feedback WHERE atom=?1 AND action='confirm_constraint'",
                    [&overflow],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(overflow_rows, 0);
        }
    }
}
