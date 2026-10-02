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
        client: "comparison-recall-test".into(),
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

fn add_observation(vault: &Vault, atom: &str, site: &str, title: &str) {
    let timestamp = now();
    vault
        .conn
        .execute(
            "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
             VALUES(?1,?2,?3,?4,NULL,'search',?5,?5,20,?6)",
            params![atom, SOURCE, site, title, timestamp, format!("comparison:{atom}")],
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

fn new_vault(root: &Path, name: &str) -> (DataDirGuard, Vault) {
    let guard = DataDirGuard::set(root);
    let mut vault = Vault::open(&root.join(name)).unwrap();
    vault.set_policy(SOURCE, &enabled_policy()).unwrap();
    (guard, vault)
}

fn lexical(vault: &mut Vault, query: &str, scope: &[&str], max_bytes: usize) -> Value {
    vault
        .recall(
            &request(query, scope, max_bytes),
            SOURCE,
            Path::new("/no/model/for/comparison-test"),
        )
        .unwrap()
}

fn hybrid(vault: &mut Vault, query: &str, max_bytes: usize, model_path: &Path) -> Value {
    vault
        .recall(
            &request(query, &["research"], max_bytes),
            SOURCE,
            model_path,
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
fn r13_separate_model_pages_contribute_in_hybrid_and_swapped_lowercase_queries() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r13.sqlite");
    let rx100 = fixture_id(1);
    let zv1 = fixture_id(2);
    let rx10 = fixture_id(3);
    add_observation(
        &vault,
        &rx100,
        "camera.example.org",
        "Sony RX100 compact camera review",
    );
    add_observation(
        &vault,
        &zv1,
        "video.example.org",
        "Sony ZV1 compact camera review",
    );
    add_observation(
        &vault,
        &rx10,
        "other-camera.example.org",
        "Sony RX10 compact camera review",
    );

    let model_path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    let refresh = vault.refresh(&model_path, 120_000).unwrap();
    assert_eq!(refresh["pending_atoms"], 0);

    for query in [
        "Compare Sony RX100 and ZV1 cameras",
        " compare sony zv1 and rx100 cameras ",
    ] {
        let packet = hybrid(&mut vault, query, 16_384, &model_path);
        let ids = context_ids(&packet);
        assert!(
            ids.contains(&rx100),
            "RX100 arm missing for {query}: {packet}"
        );
        assert!(ids.contains(&zv1), "ZV1 arm missing for {query}: {packet}");
        assert!(
            !ids.contains(&rx10),
            "RX10 must not satisfy RX100: {packet}"
        );
        assert!(
            !has_comparison_gap(&packet),
            "both arms were available: {packet}"
        );
        assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);
        assert!(serde_json::to_vec(&packet).unwrap().len() <= 16_384);
    }
}

#[test]
fn r14_measurements_versions_dates_and_ambiguous_forms_keep_single_target_gates() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r14.sqlite");

    let mac_joint = fixture_id(10);
    let mac_memory = fixture_id(11);
    let mac_storage = fixture_id(12);
    add_observation(
        &vault,
        &mac_joint,
        "laptop.example.org",
        "MacBook 16GB memory and 512GB SSD models",
    );
    add_observation(
        &vault,
        &mac_memory,
        "memory.example.org",
        "MacBook 16GB memory models",
    );
    add_observation(
        &vault,
        &mac_storage,
        "storage.example.org",
        "MacBook 512GB SSD models",
    );
    let measurement = lexical(
        &mut vault,
        "Compare 16GB and 512GB MacBook models",
        &["research"],
        4096,
    );
    assert_eq!(context_ids(&measurement), vec![mac_joint], "{measurement}");
    assert!(!has_comparison_gap(&measurement));

    let version = fixture_id(20);
    let date = fixture_id(21);
    add_observation(
        &vault,
        &version,
        "release.example.org",
        "Product v2.1 and v2.2 software release notes",
    );
    add_observation(
        &vault,
        &date,
        "calendar.example.org",
        "Release notes for 2026-09-30 and 2026-10-01",
    );
    let version_packet = lexical(
        &mut vault,
        "Compare Product v2.1 and v2.2 software releases",
        &["research"],
        4096,
    );
    assert!(
        context_ids(&version_packet).contains(&version),
        "{version_packet}"
    );
    assert!(!has_comparison_gap(&version_packet));
    let date_packet = lexical(
        &mut vault,
        "Compare releases from 2026-09-30 and 2026-10-01",
        &["research"],
        4096,
    );
    assert!(context_ids(&date_packet).contains(&date), "{date_packet}");
    assert!(!has_comparison_gap(&date_packet));

    let rx100 = fixture_id(30);
    let zv1 = fixture_id(31);
    add_observation(
        &vault,
        &rx100,
        "camera-a.example.org",
        "Sony RX100 compact camera review",
    );
    add_observation(
        &vault,
        &zv1,
        "camera-b.example.org",
        "Sony ZV1 compact camera review",
    );
    for unsupported in [
        "Compare Sony RX100 or ZV1 cameras",
        "Compare Sony RX100 and ZV1 and A6700 cameras",
    ] {
        let packet = lexical(&mut vault, unsupported, &["research"], 4096);
        assert!(
            context_ids(&packet).is_empty(),
            "ambiguous form must use single-target conjunctive constraints: {packet}"
        );
        assert!(!has_comparison_gap(&packet));
    }

    let rx100_shared = fixture_id(40);
    let zv1_shared = fixture_id(41);
    let rx100_no_memory = fixture_id(42);
    let zv1_no_usb = fixture_id(43);
    add_observation(
        &vault,
        &rx100_shared,
        "camera-a-shared.example.org",
        "Sony RX100 16GB USB-C cameras field review",
    );
    add_observation(
        &vault,
        &zv1_shared,
        "camera-b-shared.example.org",
        "Sony ZV1 16GB USB-C cameras field review",
    );
    add_observation(
        &vault,
        &rx100_no_memory,
        "camera-no-memory.example.org",
        "Sony RX100 USB-C cameras field review",
    );
    add_observation(
        &vault,
        &zv1_no_usb,
        "camera-no-cable.example.org",
        "Sony ZV1 16GB cameras field review",
    );
    let shared_constraints = lexical(
        &mut vault,
        "Compare Sony RX100 and ZV1 16GB USB-C cameras",
        &["research"],
        8192,
    );
    let ids = context_ids(&shared_constraints);
    assert!(ids.contains(&rx100_shared), "{shared_constraints}");
    assert!(ids.contains(&zv1_shared), "{shared_constraints}");
    assert!(!ids.contains(&rx100_no_memory), "{shared_constraints}");
    assert!(!ids.contains(&zv1_no_usb), "{shared_constraints}");
}

#[test]
fn r15_per_arm_lexical_capacity_prevents_one_model_distractors_from_starving_the_other() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r15-lexical-capacity.sqlite");
    let mut rx100_ids = Vec::new();
    let mut sites_by_atom = std::collections::HashMap::new();
    for index in 0..70u32 {
        let atom = fixture_id(100 + index);
        let site = format!("rx100-{index}.example.org");
        add_observation(
            &vault,
            &atom,
            &site,
            &format!("Sony RX100 cameras field review {index}"),
        );
        sites_by_atom.insert(atom.clone(), site);
        rx100_ids.push(atom);
    }
    let zv1 = fixture_id(999);
    sites_by_atom.insert(zv1.clone(), "zv1.example.org".to_owned());
    add_observation(
        &vault,
        &zv1,
        "zv1.example.org",
        "Sony ZV1 cameras field review",
    );

    let packet = lexical(
        &mut vault,
        "Compare Sony RX100 and ZV1 cameras",
        &["research"],
        16_384,
    );
    let ids = context_ids(&packet);
    assert!(
        ids.contains(&zv1),
        "the ZV1 arm has its own lexical slots: {packet}"
    );
    assert!(
        ids.iter().any(|atom| rx100_ids.contains(atom)),
        "RX100 evidence should fill its separate arm: {packet}"
    );
    assert!(
        !has_comparison_gap(&packet),
        "both arms have evidence: {packet}"
    );
    assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);
    assert!(serde_json::to_vec(&packet).unwrap().len() <= 16_384);
    let mut sites = std::collections::HashMap::<&str, usize>::new();
    for record in packet["context"].as_array().unwrap() {
        let atom = record["id"].as_str().unwrap();
        *sites.entry(sites_by_atom[atom].as_str()).or_default() += 1;
    }
    assert!(sites
        .values()
        .all(|count| *count <= algorithm::MAX_RECORDS_PER_SITE as usize));
}

#[test]
fn r15_confirmed_channels_partition_sixteen_slots_per_model_arm() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r15-confirmed-capacity.sqlite");

    let zv1 = fixture_id(900);
    add_observation(
        &vault,
        &zv1,
        "zv1.example.org",
        "Sony ZV1 cameras preference notes",
    );
    vault
        .feedback(
            SOURCE,
            &zv1,
            "confirm_constraint",
            Some("Sony ZV1 cameras preference notes"),
        )
        .unwrap();

    let mut rx100_ids = Vec::new();
    for index in 0..40u32 {
        let atom = fixture_id(100 + index);
        add_observation(
            &vault,
            &atom,
            &format!("confirmed-rx100-{index}.example.org"),
            "Sony RX100 cameras preference notes",
        );
        vault
            .feedback(
                SOURCE,
                &atom,
                "confirm_constraint",
                Some("Sony RX100 cameras preference notes"),
            )
            .unwrap();
        rx100_ids.push(atom);
    }

    let packet = lexical(
        &mut vault,
        "Compare Sony RX100 and ZV1 cameras",
        &["confirmed_preferences"],
        16_384,
    );
    let ids = context_ids(&packet);
    assert!(
        ids.contains(&zv1),
        "the confirmed ZV1 arm keeps its own slots: {packet}"
    );
    assert!(
        ids.iter().any(|atom| rx100_ids.contains(atom)),
        "confirmed RX100 evidence should also be present: {packet}"
    );
    assert!(packet["context"]
        .as_array()
        .unwrap()
        .iter()
        .all(|record| record["state"] == "confirmed"));
    assert!(!has_comparison_gap(&packet));
    assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);
    assert!(serde_json::to_vec(&packet).unwrap().len() <= 16_384);
}

#[test]
fn r15_confirmed_comparison_uses_latest_text_instead_of_original_model_title() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r15-latest-confirmation.sqlite");
    let atom = fixture_id(55);
    add_observation(
        &vault,
        &atom,
        "confirmed.example.org",
        "Sony RX100 cameras original observation",
    );
    vault
        .feedback(
            SOURCE,
            &atom,
            "confirm_constraint",
            Some("Sony RX100 cameras earlier confirmation"),
        )
        .unwrap();
    vault
        .feedback(
            SOURCE,
            &atom,
            "confirm_constraint",
            Some("Sony ZV1 cameras latest confirmation"),
        )
        .unwrap();

    let packet = lexical(
        &mut vault,
        "Compare Sony RX100 and ZV1 cameras",
        &["confirmed_preferences"],
        4096,
    );
    assert_eq!(context_ids(&packet), vec![atom], "{packet}");
    assert!(
        has_comparison_gap(&packet),
        "latest text supports ZV1 only: {packet}"
    );
}

#[test]
fn r15_same_site_capacity_tries_the_next_duplicate_representative() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r15-same-site.sqlite");

    let rx100_primary = fixture_id(1);
    let zv1_primary = fixture_id(2);
    add_observation(
        &vault,
        &rx100_primary,
        "shared.example.org",
        "Sony RX100 cameras field primary",
    );
    add_observation(
        &vault,
        &zv1_primary,
        "shared.example.org",
        "Sony ZV1 cameras field primary",
    );

    let duplicate_full_site = fixture_id(3);
    let duplicate_alternate_site = fixture_id(4);
    let duplicate_title = "Sony RX100 cameras field backup";
    add_observation(
        &vault,
        &duplicate_full_site,
        "shared.example.org",
        duplicate_title,
    );
    add_observation(
        &vault,
        &duplicate_alternate_site,
        "alternate.example.org",
        duplicate_title,
    );

    let packet = lexical(
        &mut vault,
        "Compare Sony RX100 and ZV1 cameras field",
        &["research"],
        8192,
    );
    let ids = context_ids(&packet);
    assert!(ids.contains(&rx100_primary), "{packet}");
    assert!(ids.contains(&zv1_primary), "{packet}");
    assert!(
        ids.contains(&duplicate_alternate_site),
        "the full-site duplicate is infeasible after comparison coverage, so its alternate must be selected: {packet}"
    );
    assert!(!ids.contains(&duplicate_full_site), "{packet}");
    assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);
}

#[test]
fn r15_joint_page_is_one_result_and_byte_rejection_keeps_truthful_gap() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();

    let (_joint_data_dir, mut joint_vault) = new_vault(temp.path(), "r15-joint.sqlite");
    let joint = fixture_id(1);
    add_observation(
        &joint_vault,
        &joint,
        "joint.example.org",
        "Sony RX100 and ZV1 cameras comparison review",
    );
    let joint_packet = lexical(
        &mut joint_vault,
        "Compare Sony RX100 and ZV1 cameras",
        &["research"],
        4096,
    );
    assert_eq!(context_ids(&joint_packet), vec![joint], "{joint_packet}");
    assert!(!has_comparison_gap(&joint_packet));
    assert!(joint_packet.get("comparison").is_none());

    let (_limited_data_dir, mut limited_vault) = new_vault(temp.path(), "r15-limited.sqlite");
    let long_rx100 = fixture_id(10);
    let zv1 = fixture_id(11);
    add_observation(
        &limited_vault,
        &long_rx100,
        "shared.example.org",
        &format!("Sony RX100 cameras field review {}", "filler ".repeat(450)),
    );
    add_observation(
        &limited_vault,
        &zv1,
        "shared.example.org",
        "Sony ZV1 cameras field review",
    );
    let byte_packet = lexical(
        &mut limited_vault,
        "Compare Sony RX100 and ZV1 cameras",
        &["research"],
        1800,
    );
    let ids = context_ids(&byte_packet);
    assert!(
        ids.contains(&zv1),
        "the feasible real page should survive: {byte_packet}"
    );
    assert!(
        !ids.contains(&long_rx100),
        "an oversized page must not be emitted"
    );
    assert!(
        has_comparison_gap(&byte_packet),
        "one-arm output must state its gap: {byte_packet}"
    );
    assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);
    assert!(serde_json::to_vec(&byte_packet).unwrap().len() <= 1800);
    assert!(byte_packet.get("comparison").is_none());
}

#[test]
fn r16_hybrid_direction_orders_eligible_forward_and_reverse_evidence() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r16-direction.sqlite");
    let reversed = fixture_id(2101);
    let neutral = fixture_id(2102);
    let negated_aligned = fixture_id(2103);
    let aligned = fixture_id(2104);
    add_observation(
        &vault,
        &reversed,
        "route-reversed.example.org",
        "travel route from south to north.",
    );
    add_observation(
        &vault,
        &neutral,
        "route-neutral.example.org",
        "travel route north south overview",
    );
    add_observation(
        &vault,
        &negated_aligned,
        "route-negated.example.org",
        "travel route is not from north to south.",
    );
    add_observation(
        &vault,
        &aligned,
        "route-aligned.example.org",
        "travel route from north to south.",
    );

    let model_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    let refresh = vault.refresh(&model_path, 120_000).unwrap();
    assert_eq!(refresh["pending_atoms"], 0);

    // Give the opposite-direction record the exact same stored vector so the
    // test exercises direction ordering when semantic similarity is tied.
    let reversed_vector: Vec<u8> = vault
        .conn
        .query_row(
            "SELECT vector FROM vectors WHERE atom=?1",
            [&reversed],
            |row| row.get(0),
        )
        .unwrap();
    vault
        .conn
        .execute(
            "UPDATE vectors SET vector=?1 WHERE atom=?2",
            params![&reversed_vector, &aligned],
        )
        .unwrap();
    let aligned_vector: Vec<u8> = vault
        .conn
        .query_row(
            "SELECT vector FROM vectors WHERE atom=?1",
            [&aligned],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(aligned_vector, reversed_vector);

    for query in [
        "travel route from north to south",
        "travel route from south to north",
    ] {
        let (expected_aligned, expected_reversed) = if query.ends_with("north to south") {
            (&aligned, vec![&reversed])
        } else {
            (&reversed, vec![&aligned, &negated_aligned])
        };
        for hybrid_mode in [true, false] {
            let packet = if hybrid_mode {
                hybrid(&mut vault, query, 16_384, &model_path)
            } else {
                lexical(&mut vault, query, &["research"], 16_384)
            };
            assert_eq!(
                packet["index"]["mode"],
                if hybrid_mode { "hybrid" } else { "lexical" },
                "{packet}"
            );
            let ids = context_ids(&packet);
            let position = |atom: &str| ids.iter().position(|id| id == atom).unwrap();
            assert_eq!(
                ids.len(),
                4,
                "all eligible direction evidence remains: {packet}"
            );
            assert!(
                ids.contains(expected_aligned),
                "aligned evidence missing: {packet}"
            );
            assert!(ids.contains(&neutral), "neutral evidence missing: {packet}");
            for reversed_id in &expected_reversed {
                assert!(
                    ids.contains(reversed_id),
                    "reverse evidence missing: {packet}"
                );
            }
            assert!(position(expected_aligned) < position(&neutral), "{packet}");
            if query.ends_with("north to south") {
                assert!(position(&negated_aligned) < position(&neutral), "{packet}");
            }
            for reversed_id in &expected_reversed {
                assert!(position(&neutral) < position(reversed_id), "{packet}");
            }
            assert!(
                ids.contains(&negated_aligned),
                "negated same-direction evidence must remain eligible: {packet}"
            );
        }
    }
}

#[test]
fn r17_direction_uses_latest_confirmation_text_for_ordering() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r17-confirmed-direction.sqlite");
    let replaced = fixture_id(2201);
    let neutral = fixture_id(2202);
    let aligned = fixture_id(2203);
    add_observation(
        &vault,
        &replaced,
        "confirmed-replaced.example.org",
        "travel route from north to south.",
    );
    add_observation(
        &vault,
        &neutral,
        "confirmed-neutral.example.org",
        "travel route from south to north.",
    );
    add_observation(
        &vault,
        &aligned,
        "confirmed-aligned.example.org",
        "travel route from south to north.",
    );
    for (atom, text) in [
        (&replaced, "travel route from south to north."),
        (&neutral, "travel route north south."),
        (&aligned, "travel route from north to south."),
    ] {
        vault
            .feedback(SOURCE, atom, "confirm_constraint", Some(text))
            .unwrap();
    }

    let packet = lexical(
        &mut vault,
        "travel route from north to south",
        &["confirmed_preferences"],
        16_384,
    );
    let ids = context_ids(&packet);
    assert_eq!(
        ids,
        vec![aligned.clone(), neutral.clone(), replaced.clone()],
        "{packet}"
    );
    let replaced_record = packet["context"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == replaced)
        .unwrap();
    assert_eq!(replaced_record["text"], "travel route from south to north.");
    assert!(
        packet["context"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["state"] == "confirmed")
    );
}

#[test]
fn r17_direction_phrase_in_a_facet_does_not_reorder_main_query_results() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r17-facet-direction.sqlite");
    let reversed = fixture_id(2301);
    let aligned = fixture_id(2302);
    add_observation(
        &vault,
        &reversed,
        "facet-reversed.example.org",
        "travel route from south to north overview",
    );
    add_observation(
        &vault,
        &aligned,
        "facet-aligned.example.org",
        "travel route from north to south overview",
    );
    let timestamp = now();
    vault
        .conn
        .execute(
            "UPDATE atoms SET last_seen=?1 WHERE id IN (?2,?3)",
            params![timestamp, &reversed, &aligned],
        )
        .unwrap();

    let mut recall = request("travel route", &["research"], 16_384);
    recall.facets.push("from north to south".into());
    let packet = vault
        .recall(&recall, SOURCE, Path::new("/no/model/for/direction-test"))
        .unwrap();
    let ids = context_ids(&packet);
    assert_eq!(ids, vec![reversed, aligned], "{packet}");
}

#[test]
fn r17_unsupported_multiple_to_query_keeps_fused_id_order() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let (_data_dir, mut vault) = new_vault(temp.path(), "r17-unsupported-direction.sqlite");
    let lower_id_reverse = fixture_id(2401);
    let higher_id_forward = fixture_id(2402);
    add_observation(
        &vault,
        &lower_id_reverse,
        "unsupported-reverse.example.org",
        "travel route from south to north to east.",
    );
    add_observation(
        &vault,
        &higher_id_forward,
        "unsupported-forward.example.org",
        "travel route from north to south to east.",
    );
    let timestamp = now();
    vault
        .conn
        .execute(
            "UPDATE atoms SET last_seen=?1 WHERE id IN (?2,?3)",
            params![timestamp, &lower_id_reverse, &higher_id_forward],
        )
        .unwrap();

    let packet = lexical(
        &mut vault,
        "travel route from north to south to east",
        &["research"],
        16_384,
    );
    let ids = context_ids(&packet);
    assert_eq!(ids, vec![lower_id_reverse, higher_id_forward], "{packet}");
}
