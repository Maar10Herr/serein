use rusqlite::params;
use serde_json::Value;
use serein_core::{algorithm, id, now, storage::Vault, Policy, Recall};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Mutex,
};

const SOURCE: &str = "11111111-1111-4111-8111-111111111111";
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

fn lock_data_dir() -> std::sync::MutexGuard<'static, ()> {
    DATA_DIR_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn request(query: &str, scope: &[&str], max_bytes: usize) -> Recall {
    Recall {
        protocol: 1,
        request_id: id(),
        client: "packet-selection-test".into(),
        vault: "default".into(),
        query: query.into(),
        facets: vec![],
        scope: scope.iter().map(|part| (*part).to_owned()).collect(),
        max_bytes,
        budget_ms: 2000,
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

fn add_observation(vault: &Vault, atom: &str, site: &str, title: &str, query: Option<&str>) {
    let timestamp = now();
    vault
        .conn
        .execute(
            "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
             VALUES(?1,?2,?3,?4,?5,'search',?6,?6,20,?7)",
            params![atom, SOURCE, site, title, query, timestamp, format!("packet:{atom}")],
        )
        .unwrap();
    let parsed = chrono::DateTime::parse_from_rfc3339(&timestamp).unwrap();
    vault
        .conn
        .execute(
            "INSERT INTO atom_days(atom,day,session,mass) VALUES(?1,?2,?3,1.0)",
            params![
                atom,
                parsed.format("%Y-%m-%d").to_string(),
                format!("{SOURCE}:1:{}", parsed.timestamp() / 1800)
            ],
        )
        .unwrap();
}

fn context_ids(packet: &Value) -> Vec<String> {
    packet["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}

fn recall_lexical(vault: &mut Vault, query: &str, scope: &[&str], max_bytes: usize) -> Value {
    vault
        .recall(
            &request(query, scope, max_bytes),
            SOURCE,
            Path::new("/no/model/for/lexical/recall"),
        )
        .unwrap()
}

fn new_vault(root: &Path, name: &str) -> (DataDirGuard, Vault) {
    let guard = DataDirGuard::set(root);
    let mut vault = Vault::open(&root.join(name)).unwrap();
    vault.set_policy(SOURCE, &enabled_policy()).unwrap();
    (guard, vault)
}

#[test]
fn r10_hybrid_packet_keeps_the_german_chair_witness_and_collapses_six_copies() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r10.sqlite");
    let model_path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    let german_id = fixture_id(9000);
    add_observation(
        &vault,
        &german_id,
        "research.example.org",
        "Ergonomischer Bürostuhl gegen Rückenschmerzen",
        None,
    );
    let mut cleaning_ids = Vec::new();
    for index in 0..6u32 {
        let atom = fixture_id(100 + index);
        add_observation(
            &vault,
            &atom,
            &format!("cleaning-{index}.example.org"),
            "office chair upholstery cleaning",
            None,
        );
        cleaning_ids.push(atom);
    }
    let indexed = vault.refresh(&model_path, 120_000).unwrap();
    assert_eq!(indexed["pending_atoms"], 0, "all fixture atoms indexed");

    let packet = vault
        .recall(
            &request("ergonomic office chair back pain", &["research"], 16_384),
            SOURCE,
            &model_path,
        )
        .unwrap();
    assert_eq!(packet["index"]["mode"], "hybrid");
    let ids = context_ids(&packet);
    assert!(
        ids.contains(&german_id),
        "the useful German chair evidence must remain in the packet: {packet}"
    );
    assert!(
        ids.iter().filter(|id| cleaning_ids.contains(id)).count() <= 1,
        "identical cleaning payloads occupy at most one packet slot: {packet}"
    );
    assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);

    let selected_cleaning: Vec<_> = ids
        .iter()
        .filter(|id| cleaning_ids.contains(id))
        .cloned()
        .collect();
    let explanation = vault
        .explain(SOURCE, &cleaning_ids, 4096)
        .expect("explain keeps access to the source observations");
    let explained: Vec<_> = explanation["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(explained.len(), cleaning_ids.len());
    assert_eq!(
        explained
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        6
    );
    if let Some(selected) = selected_cleaning.first() {
        assert!(explained.contains(selected));
    }
}

#[test]
fn r11_same_observed_payload_collapses_but_explain_keeps_each_real_row() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r11-duplicates.sqlite");
    let first = fixture_id(1);
    let second = fixture_id(2);
    add_observation(
        &vault,
        &first,
        "one.example.org",
        "Desk   lamp\tinstallation guide",
        Some("Desk\n lamp"),
    );
    add_observation(
        &vault,
        &second,
        "two.example.org",
        "Desk lamp installation guide",
        Some("Desk lamp"),
    );

    let packet = recall_lexical(&mut vault, "desk lamp", &["research"], 4096);
    let ids = context_ids(&packet);
    assert_eq!(ids.len(), 1, "whitespace-only differences share a slot");
    assert!(ids[0] == first || ids[0] == second);

    let explanation = vault
        .explain(SOURCE, &[first.clone(), second.clone()], 4096)
        .unwrap();
    let explained: Vec<_> = explanation["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(explained, vec![first.as_str(), second.as_str()]);
}

#[test]
fn r11_key_preserves_case_punctuation_kind_and_query_presence() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r11-exact-fields.sqlite");
    let title = "Desk lamp desk lamp reference";
    let rows = [
        (40, title, None),
        (41, "desk lamp desk lamp reference", None),
        (42, "Desk-lamp desk lamp reference", None),
        (43, title, Some("")),
        (44, "Desk lamp, desk lamp reference", None),
        (45, title, None),
    ];
    for (number, title, query) in rows {
        add_observation(
            &vault,
            &fixture_id(number),
            &format!("field-{number}.example.org"),
            title,
            query,
        );
    }
    vault
        .conn
        .execute(
            "UPDATE atoms SET kind='visit' WHERE id=?1",
            [&fixture_id(45)],
        )
        .unwrap();

    let packet = recall_lexical(&mut vault, "desk lamp", &["research"], 16_384);
    let ids = context_ids(&packet);
    for number in 40..=45 {
        assert!(
            ids.contains(&fixture_id(number)),
            "structured key field lost a distinct payload: {packet}"
        );
    }
    assert_eq!(ids.len(), 6);
}

#[test]
fn r11_payload_key_keeps_numeric_word_order_correction_and_confirmation_distinctions() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r11-distinct.sqlite");
    let rows = [
        (10, "Desk lamp 15 watt standing light", "one.example.org"),
        (11, "Desk lamp 20 watt standing light", "two.example.org"),
        (12, "Desk lamp monitor station", "three.example.org"),
        (13, "Lamp desk monitor station", "four.example.org"),
    ];
    for (number, title, site) in rows {
        add_observation(&vault, &fixture_id(number), site, title, None);
    }

    let correction_a = fixture_id(20);
    let correction_b = fixture_id(21);
    let correction_b_duplicate = fixture_id(22);
    for (atom, site) in [
        (&correction_a, "actions-a.example.org"),
        (&correction_b, "actions-b.example.org"),
        (&correction_b_duplicate, "actions-c.example.org"),
    ] {
        add_observation(
            &vault,
            atom,
            site,
            "Desk lamp setup notes",
            Some("desk lamp"),
        );
        vault.feedback(SOURCE, atom, "not_about_me", None).unwrap();
    }
    vault
        .feedback(SOURCE, &correction_a, "temporary_research", None)
        .unwrap();
    vault
        .feedback(SOURCE, &correction_b_duplicate, "not_about_me", None)
        .unwrap();

    let observed = recall_lexical(&mut vault, "desk lamp", &["research"], 16_384);
    let observed_ids = context_ids(&observed);
    for number in [10, 11, 12, 13] {
        assert!(observed_ids.contains(&fixture_id(number)), "{observed}");
    }
    assert!(observed_ids.contains(&correction_a));
    assert!(observed_ids.contains(&correction_b) || observed_ids.contains(&correction_b_duplicate));
    assert_eq!(
        observed_ids
            .iter()
            .filter(|atom| **atom == correction_b || **atom == correction_b_duplicate)
            .count(),
        1,
        "sorted unique correction actions keep equivalent histories in one group"
    );

    let mut state_vault = Vault::open(&temp.path().join("r11-states.sqlite")).unwrap();
    state_vault.set_policy(SOURCE, &enabled_policy()).unwrap();
    let observed_same = fixture_id(30);
    let confirmed_same = fixture_id(31);
    add_observation(
        &state_vault,
        &observed_same,
        "observed.example.org",
        "Desk lamp preference notes",
        Some("desk lamp"),
    );
    add_observation(
        &state_vault,
        &confirmed_same,
        "confirmed.example.org",
        "Desk lamp preference notes",
        Some("desk lamp"),
    );
    state_vault
        .feedback(
            SOURCE,
            &confirmed_same,
            "confirm_constraint",
            Some("desk lamp preference notes"),
        )
        .unwrap();
    let both_states = recall_lexical(
        &mut state_vault,
        "desk lamp",
        &["research", "confirmed_preferences"],
        16_384,
    );
    let both_ids = context_ids(&both_states);
    assert!(both_ids.contains(&observed_same), "{both_states}");
    assert!(both_ids.contains(&confirmed_same), "{both_states}");
    assert_eq!(
        both_states["context"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["id"] == confirmed_same)
            .count(),
        1
    );
}

#[test]
fn r12_site_and_byte_rejections_try_the_next_feasible_group_representative() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r12.sqlite");

    let filler_a = fixture_id(1);
    let filler_b = fixture_id(2);
    add_observation(
        &vault,
        &filler_a,
        "full.example.org",
        "Desk lamp desk lamp desk lamp setup guide",
        None,
    );
    add_observation(
        &vault,
        &filler_b,
        "full.example.org",
        "Desk lamp desk lamp desk lamp lighting tips",
        None,
    );
    let full_site_duplicate = fixture_id(3);
    let alternate_site_duplicate = fixture_id(4);
    add_observation(
        &vault,
        &full_site_duplicate,
        "full.example.org",
        "Desk lamp assembly manual",
        None,
    );
    add_observation(
        &vault,
        &alternate_site_duplicate,
        "alternate.example.org",
        "Desk lamp assembly manual",
        None,
    );
    let packet = recall_lexical(&mut vault, "desk lamp", &["research"], 16_384);
    let ids = context_ids(&packet);
    assert!(
        ids.contains(&filler_a) && ids.contains(&filler_b),
        "{packet}"
    );
    assert!(
        !ids.contains(&full_site_duplicate),
        "the full-site representative is infeasible"
    );
    assert!(
        ids.contains(&alternate_site_duplicate),
        "the selector should choose the next feasible real representative: {packet}"
    );

    let mut byte_vault = Vault::open(&temp.path().join("r12-byte.sqlite")).unwrap();
    byte_vault.set_policy(SOURCE, &enabled_policy()).unwrap();
    let byte_filler = fixture_id(9);
    add_observation(
        &byte_vault,
        &byte_filler,
        "filler.example.org",
        "Desk lamp desk lamp desk lamp desk lamp desk lamp support guide",
        None,
    );
    let long_whitespace = fixture_id(10);
    let compact_duplicate = fixture_id(11);
    add_observation(
        &byte_vault,
        &long_whitespace,
        "long.example.org",
        &format!("Desk{}lamp installation guide", " ".repeat(220)),
        None,
    );
    add_observation(
        &byte_vault,
        &compact_duplicate,
        "compact.example.org",
        "Desk lamp installation guide",
        None,
    );
    // Size the narrow request from the current wire metadata. Required limits
    // text can grow without changing the case: the compact representative must
    // fit, while the whitespace-heavy representative must remain infeasible.
    let wide_packet = recall_lexical(&mut byte_vault, "desk lamp", &["research"], 4096);
    let mut compact_packet = wide_packet.clone();
    let representative = compact_packet["context"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["id"] == long_whitespace)
        .expect("the wider request must include the first duplicate representative");
    representative["id"] = serde_json::json!(compact_duplicate);
    representative["text"] = serde_json::json!(
        "Observed page title on compact.example.org: Desk lamp installation guide"
    );
    let byte_budget = serde_json::to_vec(&compact_packet).unwrap().len() + 64;
    assert!(byte_budget < serde_json::to_vec(&wide_packet).unwrap().len());
    assert!(byte_budget < 4096);
    let byte_packet = recall_lexical(&mut byte_vault, "desk lamp", &["research"], byte_budget);
    let byte_ids = context_ids(&byte_packet);
    assert!(
        byte_ids.contains(&byte_filler),
        "the prefix candidate should fit: {byte_packet}"
    );
    assert!(
        !byte_ids.contains(&long_whitespace),
        "oversized representative must be skipped"
    );
    assert!(
        byte_ids.contains(&compact_duplicate),
        "byte rejection must not consume the duplicate group: {byte_packet}"
    );
    assert!(serde_json::to_vec(&byte_packet).unwrap().len() <= byte_budget);
}
