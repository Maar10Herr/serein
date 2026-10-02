use rusqlite::params;
use serein_core::{algorithm, id, model, storage::Vault, Event, Policy};
use std::{ffi::OsString, fs, path::Path, sync::Mutex};

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

fn test_policy(capture_epoch: u64) -> Policy {
    Policy {
        consent: true,
        recall_enabled: true,
        capture_epoch,
        ..Policy::default()
    }
}

fn axis_bytes() -> Vec<u8> {
    let mut vector = vec![0.0_f32; algorithm::DIMENSIONS];
    vector[0] = 1.0;
    vector
        .into_iter()
        .flat_map(|component| component.to_le_bytes())
        .collect()
}

fn orthogonal_bytes() -> Vec<u8> {
    let mut vector = vec![0.0_f32; algorithm::DIMENSIONS];
    vector[1] = 1.0;
    vector
        .into_iter()
        .flat_map(|component| component.to_le_bytes())
        .collect()
}

fn ingest_visit(vault: &mut Vault, source: &str, site: &str, title: &str, hours_ago: i64) {
    let observed_at = (chrono::Utc::now() - chrono::Duration::hours(hours_ago))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let result = vault
        .ingest(
            source,
            1,
            vec![Event {
                event_id: id(),
                visit_id: id(),
                site_key: site.to_owned(),
                site_epoch: 0,
                observed_at,
                kind: "visit".to_owned(),
                title: title.to_owned(),
                search_query: None,
                foreground_seconds: 120,
            }],
        )
        .unwrap();
    assert_eq!(
        result["acknowledged_ids"].as_array().unwrap().len(),
        1,
        "the fixture observation must be accepted"
    );
}

#[test]
fn dashboard_group_labels_follow_current_source_and_site_policy() {
    let _data_dir_lock = DATA_DIR_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(directory.path());
    let model_dir = directory.path().join("models/current");
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
    let database = directory.path().join("dashboard-topic-privacy.sqlite");
    let mut vault = Vault::open(&database).unwrap();
    let source = id();
    let other_source = id();
    vault.set_policy(&source, &test_policy(1)).unwrap();
    vault.set_policy(&other_source, &test_policy(1)).unwrap();

    let entries = [
        ("allowed.example", "Allowed root chair sizing guide", 5),
        (
            "child.allowed.example",
            "Allowed subdomain chair dimensions guide",
            4,
        ),
        (
            "allowed-extra.example",
            "Allowed single-item chair comfort notes",
            8,
        ),
        (
            "blocked.example",
            "FORBIDDEN excluded root private research title",
            3,
        ),
        (
            "child.blocked.example",
            "FORBIDDEN excluded subdomain private research title",
            2,
        ),
        (
            "outside.example",
            "FORBIDDEN unselected site private research title",
            6,
        ),
    ];
    for (site, title, hours_ago) in entries {
        ingest_visit(&mut vault, &source, site, title, hours_ago);
    }
    ingest_visit(
        &mut vault,
        &other_source,
        "other.example",
        "FORBIDDEN other source global topic label",
        36,
    );
    let retained_source_atoms: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM atoms WHERE source=?",
            [&source],
            |row| row.get(0),
        )
        .unwrap();
    let retained_other_source_atoms: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM atoms WHERE source=?",
            [&other_source],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_source_atoms, 6);
    assert_eq!(retained_other_source_atoms, 1);

    // Narrow policy after capture so retained historical rows exercise the
    // dashboard's current selected-site and excluded-site label gate.
    vault
        .set_policy(
            &source,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 2,
                selected_only: true,
                // Include the blocked host in the selected set so this test
                // exercises the independent exclusion gate as well.
                selected_sites: vec![
                    "allowed.example".to_owned(),
                    "allowed-extra.example".to_owned(),
                    "blocked.example".to_owned(),
                ],
                excluded_sites: vec!["blocked.example".to_owned()],
                ..Policy::default()
            },
        )
        .unwrap();

    let topic = id();
    vault
        .conn
        .execute(
            "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
            params![
                topic,
                "FORBIDDEN stale global label",
                axis_bytes(),
                model_hash
            ],
        )
        .unwrap();
    let extra_topic = id();
    let other_source_topic = id();
    for (topic_id, label) in [
        (&extra_topic, "FORBIDDEN stale extra label"),
        (&other_source_topic, "FORBIDDEN source-only stale label"),
    ] {
        vault
            .conn
            .execute(
                "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
                params![topic_id, label, axis_bytes(), model_hash],
            )
            .unwrap();
    }
    let atoms = vault
        .conn
        .prepare("SELECT id,source,site FROM atoms ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (atom, atom_source, site) in atoms {
        // Make the global activity label deterministically prefer one of the
        // policy-ineligible labels, then verify the dashboard scopes it.
        let vector = if atom_source == other_source
            || matches!(
                site.as_str(),
                "blocked.example" | "child.blocked.example" | "outside.example"
            ) {
            axis_bytes()
        } else {
            orthogonal_bytes()
        };
        vault
            .conn
            .execute(
                "INSERT INTO vectors(atom,model,vector) VALUES(?,?,?)",
                params![atom, model_hash, vector],
            )
            .unwrap();
        let assigned_topic = if site == "allowed-extra.example" {
            &extra_topic
        } else {
            &topic
        };
        vault
            .conn
            .execute(
                "INSERT INTO atom_topics(atom,topic,mass) VALUES(?,?,1.0)",
                params![atom, assigned_topic],
            )
            .unwrap();
        if atom_source == other_source {
            vault
                .conn
                .execute(
                    "INSERT INTO atom_topics(atom,topic,mass) VALUES(?,?,1.0)",
                    params![atom, other_source_topic],
                )
                .unwrap();
        }
    }

    let global_topics = serein_core::inference::activity(&vault.conn, Some(&model_hash)).unwrap();
    let global_topics = global_topics.as_array().unwrap();
    assert_eq!(global_topics.len(), 3);
    let global_topic = global_topics
        .iter()
        .find(|candidate| candidate["id"] == topic)
        .unwrap();
    assert!(global_topic["label"]
        .as_str()
        .unwrap_or_default()
        .contains("FORBIDDEN"));

    let dashboard = vault.dashboard(&source).unwrap();
    let cards = dashboard["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 6, "owner evidence cards remain available");
    for site in [
        "blocked.example",
        "child.blocked.example",
        "outside.example",
    ] {
        let card = cards
            .iter()
            .find(|card| card["site"] == site)
            .unwrap_or_else(|| panic!("raw evidence card for {site} remains available"));
        assert!(card["text"]
            .as_str()
            .unwrap_or_default()
            .contains("FORBIDDEN"));
    }

    let memories = dashboard["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1);
    let memory = &memories[0];
    let items = memory["items"].as_array().unwrap();
    assert_eq!(memory["evidence_count"], 2);
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| item["site"] == "allowed.example"));
    assert!(items
        .iter()
        .any(|item| item["site"] == "child.allowed.example"));
    assert!(items.iter().all(|item| {
        item["site"] != "blocked.example" && item["site"] != "child.blocked.example"
    }));
    assert!([
        "Allowed root chair sizing guide",
        "Allowed subdomain chair dimensions guide",
    ]
    .contains(&memory["label"].as_str().unwrap()));
    let dashboard_topics = dashboard["topics"].as_array().unwrap();
    assert_eq!(dashboard_topics.len(), 2);
    let dashboard_topic = dashboard_topics
        .iter()
        .find(|candidate| candidate["id"] == topic)
        .unwrap();
    let extra_dashboard_topic = dashboard_topics
        .iter()
        .find(|candidate| candidate["id"] == extra_topic)
        .unwrap();
    assert_eq!(dashboard_topic["label"], memory["label"]);
    assert_eq!(dashboard_topic["sessions"], 2);
    assert_eq!(dashboard_topic["sites"], 2);
    assert_eq!(extra_dashboard_topic["sessions"], 1);
    assert_eq!(extra_dashboard_topic["sites"], 1);
    assert_eq!(extra_dashboard_topic["candidate"], false);
    assert!(!dashboard_topics
        .iter()
        .any(|candidate| candidate["id"] == other_source_topic));

    assert!(global_topic["sessions"].as_u64().unwrap() > 2);
    assert!(global_topic["sites"].as_u64().unwrap() > 2);
    for index in 0..3 {
        let global_mass = global_topic["mass"][index].as_f64().unwrap();
        let scoped_mass = dashboard_topic["mass"][index].as_f64().unwrap();
        assert!(global_mass > scoped_mass);
        let global_share = global_topic["activity_share"][index].as_f64().unwrap();
        let scoped_share = dashboard_topic["activity_share"][index].as_f64().unwrap();
        assert!((global_share - scoped_share).abs() > 1.0e-6);
    }
    assert!(
        (global_topic["burst"].as_f64().unwrap() - dashboard_topic["burst"].as_f64().unwrap())
            .abs()
            > 1.0e-6
    );

    let forbidden = [
        "FORBIDDEN excluded root private research title",
        "FORBIDDEN excluded subdomain private research title",
        "FORBIDDEN unselected site private research title",
        "FORBIDDEN other source global topic label",
        "FORBIDDEN stale global label",
        "FORBIDDEN stale extra label",
        "FORBIDDEN source-only stale label",
    ];
    for label in std::iter::once(memory["label"].as_str().unwrap()).chain(
        dashboard["topics"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|topic| topic["label"].as_str()),
    ) {
        for text in forbidden {
            assert!(
                !label.contains(text),
                "grouped label leaked ineligible text {text:?}: {label:?}"
            );
        }
    }
    for item in items {
        let text = item["text"].as_str().unwrap_or_default();
        for forbidden_text in forbidden {
            assert!(!text.contains(forbidden_text));
        }
    }
}
