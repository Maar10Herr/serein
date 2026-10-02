use rusqlite::params;
use serein_core::{algorithm, id, inference, now, storage::Vault, Event, Policy, Recall};
use std::collections::{BTreeMap, HashSet};

fn enabled_policy(capture_epoch: u64) -> Policy {
    Policy {
        consent: true,
        recall_enabled: true,
        capture_epoch,
        ..Policy::default()
    }
}

fn event(observed_at: &str) -> Event {
    Event {
        event_id: id(),
        visit_id: id(),
        site_key: "research.example.org".into(),
        site_epoch: 0,
        observed_at: observed_at.into(),
        kind: "search".into(),
        title: "Research on activity windows".into(),
        search_query: Some("activity window research".into()),
        foreground_seconds: 30,
    }
}

fn timestamp(seconds: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn activity_window_keys(vault: &Vault, atom: &str) -> HashSet<String> {
    let mut statement = vault
        .conn
        .prepare("SELECT session FROM atom_days WHERE atom=? ORDER BY session")
        .unwrap();
    statement
        .query_map([atom], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<HashSet<_>, _>>()
        .unwrap()
}

#[test]
fn two_second_boundary_and_capture_epoch_each_add_an_activity_window() {
    let temp = tempfile::tempdir().unwrap();
    let mut vault = Vault::open(&temp.path().join("vault.sqlite")).unwrap();
    let source = id();
    vault.set_policy(&source, &enabled_policy(1)).unwrap();
    let boundary = chrono::Utc::now().timestamp().div_euclid(1800) * 1800;
    let before = timestamp(boundary - 1);
    let after = timestamp(boundary + 1);
    let first = (boundary - 1).div_euclid(1800);
    let second = (boundary + 1).div_euclid(1800);
    assert_eq!(
        second,
        first + 1,
        "two timestamps straddle a bucket boundary"
    );

    let result = vault
        .ingest(&source, 1, vec![event(&before), event(&after)])
        .unwrap();
    assert_eq!(result["acknowledged_ids"].as_array().unwrap().len(), 2);
    let atom: String = vault
        .conn
        .query_row("SELECT id FROM atoms WHERE source=?", [&source], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        activity_window_keys(&vault, &atom),
        HashSet::from([
            format!("{source}:1:{first}"),
            format!("{source}:1:{second}"),
        ]),
        "two seconds across the boundary count as two activity windows"
    );

    vault.set_policy(&source, &enabled_policy(2)).unwrap();
    let result = vault.ingest(&source, 2, vec![event(&after)]).unwrap();
    assert_eq!(result["acknowledged_ids"].as_array().unwrap().len(), 1);
    let windows = activity_window_keys(&vault, &atom);
    assert_eq!(windows.len(), 3);
    assert!(windows.contains(&format!("{source}:2:{second}")));
    let distinct_activity_windows: i64 = vault
        .conn
        .query_row(
            "SELECT count(DISTINCT session) FROM atom_days WHERE atom=?",
            [&atom],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(distinct_activity_windows, 3);

    let packet = vault
        .recall(
            &Recall {
                protocol: 1,
                request_id: id(),
                client: "activity-window-test".into(),
                vault: "default".into(),
                query: "activity window research".into(),
                facets: vec![],
                scope: vec!["research".into()],
                max_bytes: 4096,
                budget_ms: 2000,
            },
            &source,
            &temp.path().join("missing-model"),
        )
        .unwrap();
    assert_eq!(packet["context"].as_array().unwrap().len(), 1);
    let record = &packet["context"][0];
    assert_eq!(record["evidence"]["sessions"], 3);
    assert!(record["limits"].as_array().unwrap().iter().any(|limit| {
        limit == "Activity counts use fixed 30-minute windows; they are not independent confirmations."
    }));
}

fn membership_partition(arrival_order: &[usize]) -> [bool; 3] {
    let temp = tempfile::tempdir().unwrap();
    let vault = Vault::open(&temp.path().join("vault.sqlite")).unwrap();
    let model_hash = "synthetic-order-witness";
    let mut ids = Vec::new();
    for (index, degrees) in [0.0_f32, 50.0, 100.0].into_iter().enumerate() {
        let radians = degrees.to_radians();
        let mut vector = vec![0.0; algorithm::DIMENSIONS];
        vector[0] = radians.cos();
        vector[1] = radians.sin();
        let atom = format!("00000000-0000-4000-8000-{index:012}");
        vault
            .conn
            .execute(
                "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
                 VALUES(?1,'source','example.org',?2,NULL,'visit',?3,?3,30,?4)",
                params![atom, format!("Synthetic direction {index}"), now(), format!("witness-{index}")],
            )
            .unwrap();
        vault
            .conn
            .execute(
                "INSERT INTO atom_days(atom,day,session,mass) VALUES(?1,'2026-09-30','source:1:1',1.0)",
                [&atom],
            )
            .unwrap();
        let vector_bytes: Vec<u8> = vector
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        vault
            .conn
            .execute(
                "INSERT INTO vectors(atom,model,vector) VALUES(?1,?2,?3)",
                params![atom, model_hash, vector_bytes],
            )
            .unwrap();
        ids.push((atom, vector));
    }

    for &index in arrival_order {
        inference::assign(
            &vault.conn,
            &ids[index].0,
            &ids[index].1,
            &format!("Synthetic direction {index}"),
            model_hash,
        )
        .unwrap();
    }

    let mut statement = vault
        .conn
        .prepare("SELECT atom,topic FROM atom_topics")
        .unwrap();
    let memberships: BTreeMap<String, String> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let topic_by_index: Vec<_> = ids
        .iter()
        .map(|(atom, _)| memberships.get(atom).unwrap())
        .collect();
    [
        topic_by_index[0] == topic_by_index[1],
        topic_by_index[0] == topic_by_index[2],
        topic_by_index[1] == topic_by_index[2],
    ]
}

#[test]
fn synthetic_zero_fifty_hundred_degree_topic_memberships_depend_on_arrival_order() {
    // This function-level synthetic-vector witness does not claim that natural
    // page titles produce these exact vectors.
    assert_eq!(membership_partition(&[0, 1, 2]), [true, false, false]);
    assert_eq!(membership_partition(&[1, 2, 0]), [false, false, true]);
}
