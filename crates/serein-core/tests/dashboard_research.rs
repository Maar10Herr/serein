use rusqlite::params;
use serein_core::{algorithm, id, model, now, storage::Vault, Event, Policy};
use std::fs;

static DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn assigned_vector_blob() -> Vec<u8> {
    let mut vector = vec![0.0_f32; algorithm::DIMENSIONS];
    vector[0] = 1.0;
    vector
        .into_iter()
        .flat_map(|component| component.to_le_bytes())
        .collect()
}

#[test]
fn dashboard_promotes_related_research_without_hiding_raw_activity() {
    let _data_dir_lock = DATA_DIR_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
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
    std::env::set_var("SEREIN_DATA_DIR", temp.path());
    let model_hash = model::current_hash(&model_dir).unwrap();
    let mut vault = Vault::open(&temp.path().join("dashboard.sqlite")).unwrap();
    let source = id();
    vault
        .set_policy(
            &source,
            &Policy {
                consent: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    for (site, title, seconds) in [
        ("x.com", "Home / X", 300),
        ("chairs.example", "Ergonomic office chair sizing guide", 120),
        (
            "reviews.example",
            "Ergonomic office chair dimensions review",
            90,
        ),
    ] {
        vault
            .ingest(
                &source,
                1,
                vec![Event {
                    event_id: id(),
                    visit_id: id(),
                    site_key: site.into(),
                    site_epoch: 0,
                    observed_at: now(),
                    kind: "visit".into(),
                    title: title.into(),
                    search_query: None,
                    foreground_seconds: seconds,
                }],
            )
            .unwrap();
    }
    vault
        .ingest(
            &source,
            1,
            vec![Event {
                event_id: id(),
                visit_id: id(),
                site_key: "google.com".into(),
                site_epoch: 0,
                observed_at: now(),
                kind: "search".into(),
                title: "Google".into(),
                search_query: Some("ergonomic office chair dimensions".into()),
                foreground_seconds: 30,
            }],
        )
        .unwrap();
    // Put the useful cluster behind the raw-card window while keeping it
    // comfortably inside the dashboard's 90-day evidence period.
    let older_seen = (chrono::Utc::now() - chrono::Duration::days(8))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    vault
        .conn
        .execute(
            "UPDATE atoms SET last_seen=? WHERE site IN ('chairs.example','reviews.example','google.com')",
            [older_seen],
        )
        .unwrap();
    for i in 0..75 {
        vault
            .ingest(
                &source,
                1,
                vec![Event {
                    event_id: id(),
                    visit_id: id(),
                    site_key: format!("noise-{i}.example"),
                    site_epoch: 0,
                    observed_at: now(),
                    kind: "visit".into(),
                    title: "Home / X".into(),
                    search_query: None,
                    foreground_seconds: 30,
                }],
            )
            .unwrap();
    }
    let topic = id();
    vault
        .conn
        .execute(
            "INSERT INTO topics(id,label,centroid,model) VALUES(?1,?2,?3,?4)",
            params![
                topic,
                "Stale topic label from removed evidence",
                vec![0u8; algorithm::DIMENSIONS * 4],
                model_hash
            ],
        )
        .unwrap();
    let atoms = vault
        .conn
        .prepare("SELECT id FROM atoms ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for atom in atoms {
        vault
            .conn
            .execute(
                "INSERT INTO vectors(atom,model,vector) VALUES(?,?,?)",
                params![atom, model_hash, assigned_vector_blob()],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_topics(atom,topic,mass) VALUES(?1,?2,1)",
                params![atom, topic],
            )
            .unwrap();
    }
    let dashboard = vault.dashboard(&source).unwrap();
    let cards = dashboard["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 60, "raw activity remains capped at 60 cards");
    assert!(cards
        .iter()
        .all(|card| !["chairs.example", "reviews.example", "google.com"]
            .contains(&card["site"].as_str().unwrap())));
    let memories = dashboard["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0]["evidence_count"], 3);
    assert_ne!(
        memories[0]["label"],
        "Stale topic label from removed evidence"
    );
    assert_eq!(memories[0]["items"].as_array().unwrap().len(), 3);
    assert!(memories[0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| !item["site"].as_str().unwrap().starts_with("noise-")));
}

#[test]
fn dashboard_keeps_corrected_activity_but_excludes_it_from_memories_and_prominence() {
    let _data_dir_lock = DATA_DIR_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
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
    std::env::set_var("SEREIN_DATA_DIR", temp.path());
    let model_hash = model::current_hash(&model_dir).unwrap();
    let mut vault = Vault::open(&temp.path().join("dashboard-corrections.sqlite")).unwrap();
    let source = id();
    vault
        .set_policy(
            &source,
            &Policy {
                consent: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .unwrap();
    let searches = [
        ("do-not-use.example", "do_not_use"),
        ("not-about-me.example", "not_about_me"),
        ("temporary.example", "temporary_research"),
        ("eligible.example", ""),
    ];
    for (site, _) in searches {
        vault
            .ingest(
                &source,
                1,
                vec![Event {
                    event_id: id(),
                    visit_id: id(),
                    site_key: site.into(),
                    site_epoch: 0,
                    observed_at: now(),
                    kind: "search".into(),
                    title: "Google".into(),
                    search_query: Some("ergonomic office chair dimensions".into()),
                    foreground_seconds: 30,
                }],
            )
            .unwrap();
    }
    let topic = id();
    vault
        .conn
        .execute(
            "INSERT INTO topics(id,label,centroid,model) VALUES(?1,?2,?3,?4)",
            params![
                topic,
                "Ergonomic office chairs",
                vec![0u8; algorithm::DIMENSIONS * 4],
                model_hash
            ],
        )
        .unwrap();
    let atoms = vault
        .conn
        .prepare("SELECT id,site FROM atoms ORDER BY site")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (atom, site) in &atoms {
        vault
            .conn
            .execute(
                "INSERT INTO vectors(atom,model,vector) VALUES(?,?,?)",
                params![atom, model_hash, assigned_vector_blob()],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_topics(atom,topic,mass) VALUES(?1,?2,1)",
                params![atom, topic],
            )
            .unwrap();
        if let Some((_, action)) = searches.iter().find(|(candidate, _)| candidate == site) {
            if !action.is_empty() {
                vault.feedback(&source, atom, action, None).unwrap();
            }
        }
    }

    let dashboard = vault.dashboard(&source).unwrap();
    let cards = dashboard["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 4, "corrected observations remain raw cards");
    for (site, action) in searches.iter().filter(|(_, action)| !action.is_empty()) {
        let card = cards.iter().find(|card| card["site"] == *site).unwrap();
        assert_eq!(card["prominent"], false, "{site} is not prominent");
        assert!(card["corrections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|correction| correction["action"] == *action));
    }
    assert_eq!(
        dashboard["memories"].as_array().unwrap().len(),
        0,
        "one remaining eligible item is not a research memory"
    );
}
