use rusqlite::{params, Connection};
use serein_core::{algorithm, inference, model, now, storage::Vault};
use std::{collections::BTreeMap, path::Path};

fn fixture() -> (tempfile::TempDir, Vault) {
    let directory = tempfile::tempdir().unwrap();
    let vault = Vault::open(&directory.path().join("topic-cache.sqlite")).unwrap();
    (directory, vault)
}

fn axis(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0; algorithm::DIMENSIONS];
    vector[index] = 1.0;
    vector
}

fn bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|component| component.to_le_bytes())
        .collect()
}

fn add_atom(vault: &Vault, id: &str, site: &str, vector: &[f32], days: &[(&str, &str, f64)]) {
    vault
        .conn
        .execute(
            "INSERT INTO atoms VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![
                id,
                "topic-cache-test",
                site,
                format!("Desk lamp installation {id}"),
                Option::<String>::None,
                "visit",
                format!("2026-09-{:02}T00:00:00Z", id.len().max(1)),
                now(),
                1,
                format!("canonical-{id}")
            ],
        )
        .unwrap();
    for (day, session, mass) in days {
        vault
            .conn
            .execute(
                "INSERT INTO atom_days VALUES(?,?,?,?)",
                params![id, day, session, mass],
            )
            .unwrap();
    }
    vault
        .conn
        .execute(
            "INSERT INTO vectors VALUES(?,?,?)",
            params![id, "model", bytes(vector)],
        )
        .unwrap();
}

fn assign(vault: &Vault, atom: &str, vector: &[f32]) {
    inference::assign(&vault.conn, atom, vector, "Desk lamp installation", "model").unwrap();
}

fn assigned_topic(vault: &Vault, atom: &str) -> String {
    vault
        .conn
        .query_row(
            "SELECT topic FROM atom_topics WHERE atom=? ORDER BY mass DESC,topic LIMIT 1",
            [atom],
            |row| row.get(0),
        )
        .unwrap()
}

#[derive(Default)]
struct RefBucket {
    mass: f64,
    resultant: Vec<f64>,
}

fn reference_topic(conn: &Connection, topic: &str) -> BTreeMap<(String, String), RefBucket> {
    reference_topic_except(conn, topic, None)
}

fn reference_topic_except(
    conn: &Connection,
    topic: &str,
    excluded_atom: Option<&str>,
) -> BTreeMap<(String, String), RefBucket> {
    let mut statement = conn
        .prepare(
            "SELECT a.site,d.session,d.mass,m.mass,v.vector
             FROM atom_topics m
             JOIN atoms a ON a.id=m.atom
             JOIN vectors v ON v.atom=a.id AND v.model='model'
             JOIN atom_days d ON d.atom=a.id
             WHERE m.topic=? AND (?2 IS NULL OR m.atom<>?2)
             ORDER BY a.site,d.session,a.id,d.day",
        )
        .unwrap();
    let mut rows = statement.query(params![topic, excluded_atom]).unwrap();
    let mut buckets = BTreeMap::<(String, String), RefBucket>::new();
    while let Some(row) = rows.next().unwrap() {
        let key = (row.get(0).unwrap(), row.get(1).unwrap());
        let day_mass: f64 = row.get(2).unwrap();
        let membership_mass: f64 = row.get(3).unwrap();
        let blob: Vec<u8> = row.get(4).unwrap();
        assert_eq!(blob.len(), algorithm::DIMENSIONS * 4);
        let bucket = buckets.entry(key).or_default();
        if bucket.resultant.is_empty() {
            bucket.resultant.resize(algorithm::DIMENSIONS, 0.0);
        }
        let weight = day_mass * membership_mass;
        bucket.mass += weight;
        for (sum, component) in bucket.resultant.iter_mut().zip(blob.chunks_exact(4)) {
            *sum += weight * f32::from_le_bytes(component.try_into().unwrap()) as f64;
        }
    }
    buckets
}

fn reference_sum(buckets: &BTreeMap<(String, String), RefBucket>) -> Vec<f64> {
    let mut sum = vec![0.0; algorithm::DIMENSIONS];
    for bucket in buckets.values() {
        if bucket.mass > 0.0 {
            let scale = bucket.mass.min(1.0) / bucket.mass;
            for (total, component) in sum.iter_mut().zip(&bucket.resultant) {
                *total += component * scale;
            }
        }
    }
    sum
}

fn reference_assignments(conn: &Connection, vector: &[f32]) -> Vec<(String, f64)> {
    reference_assignments_except(conn, vector, None)
}

fn reference_assignments_except(
    conn: &Connection,
    vector: &[f32],
    excluded_atom: Option<&str>,
) -> Vec<(String, f64)> {
    let topic_ids = conn
        .prepare("SELECT id FROM topics WHERE model='model' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut candidates = topic_ids
        .into_iter()
        .filter_map(|topic| {
            let sum = reference_sum(&reference_topic_except(conn, &topic, excluded_atom));
            let centroid = model::normalize(sum.into_iter().map(|value| value as f32).collect())?;
            Some((topic, model::cosine(&centroid, vector)))
        })
        .filter(|(_, score)| score.is_finite())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    let Some((first_topic, first_score)) = candidates.first() else {
        return Vec::new();
    };
    if *first_score < algorithm::TOPIC_ADMISSION {
        return Vec::new();
    }
    if let Some((second_topic, second_score)) = candidates.get(1) {
        if first_score - second_score < algorithm::TOPIC_MARGIN
            && *second_score >= algorithm::RUNNER_UP_ADMISSION
        {
            let first = (10.0 * f64::from(*first_score)).exp();
            let second = (10.0 * f64::from(*second_score)).exp();
            return vec![
                (first_topic.clone(), first / (first + second)),
                (second_topic.clone(), second / (first + second)),
            ];
        }
    }
    vec![(first_topic.clone(), 1.0)]
}

fn assert_assigned_like_reference(vault: &Vault, atom: &str, expected: &[(String, f64)]) {
    let actual = vault
        .conn
        .prepare("SELECT topic,mass FROM atom_topics WHERE atom=? ORDER BY topic")
        .unwrap()
        .query_map([atom], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    if expected.is_empty() {
        assert_eq!(
            actual.len(),
            1,
            "no reference candidate should create a topic"
        );
        return;
    }
    let mut expected = expected.to_vec();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(actual.len(), expected.len());
    for ((actual_topic, actual_mass), (expected_topic, expected_mass)) in
        actual.iter().zip(&expected)
    {
        assert_eq!(actual_topic, expected_topic);
        assert!((actual_mass - expected_mass).abs() <= 1.0e-12);
    }
}

fn assert_cache_matches_reference(vault: &Vault, topic: &str) {
    let buckets = reference_topic(&vault.conn, topic);
    let expected_sum = reference_sum(&buckets);
    let cache: (String, i64, Vec<u8>) = vault
        .conn
        .query_row(
            "SELECT model,valid,sum FROM topic_cache WHERE topic=?",
            [topic],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(cache.0, "model");
    assert_eq!(cache.1, 1, "cached accumulator must be valid");
    assert_eq!(cache.2.len(), algorithm::DIMENSIONS * 8);
    let actual_sum: Vec<_> = cache
        .2
        .chunks_exact(8)
        .map(|component| f64::from_le_bytes(component.try_into().unwrap()))
        .collect();
    for (actual, expected) in actual_sum.iter().zip(&expected_sum) {
        assert!(
            (actual - expected).abs() <= 1.0e-9,
            "S mismatch: {actual} vs {expected}"
        );
    }

    let actual_buckets: BTreeMap<_, _> = vault
        .conn
        .prepare(
            "SELECT site,session,mass,resultant FROM topic_cache_buckets
             WHERE topic=? ORDER BY site,session",
        )
        .unwrap()
        .query_map([topic], |row| {
            let mass: f64 = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            let resultant = blob
                .chunks_exact(8)
                .map(|component| f64::from_le_bytes(component.try_into().unwrap()))
                .collect::<Vec<_>>();
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                (mass, resultant),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let expected_keys: BTreeMap<_, _> = buckets
        .into_iter()
        .filter(|(_, bucket)| bucket.mass > 0.0)
        .map(|(key, bucket)| (key, (bucket.mass, bucket.resultant)))
        .collect();
    assert_eq!(actual_buckets.len(), expected_keys.len());
    for (key, (mass, resultant)) in expected_keys {
        let (actual_mass, actual_resultant) = &actual_buckets[&key];
        assert!(
            (actual_mass - mass).abs() <= 1.0e-9,
            "M mismatch for {key:?}"
        );
        for (actual, expected) in actual_resultant.iter().zip(resultant) {
            assert!(
                (actual - expected).abs() <= 1.0e-9,
                "R mismatch for {key:?}"
            );
        }
    }

    let expected_centroid = model::normalize(
        expected_sum
            .into_iter()
            .map(|component| component as f32)
            .collect(),
    );
    if let Some(expected_centroid) = expected_centroid {
        let blob: Vec<u8> = vault
            .conn
            .query_row("SELECT centroid FROM topics WHERE id=?", [topic], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(blob.len(), algorithm::DIMENSIONS * 4);
        let actual = blob
            .chunks_exact(4)
            .map(|component| f32::from_le_bytes(component.try_into().unwrap()))
            .collect::<Vec<_>>();
        let l2 = actual
            .iter()
            .zip(expected_centroid)
            .map(|(actual, expected)| (actual - expected).powi(2))
            .sum::<f32>()
            .sqrt();
        assert!(l2 <= 1.0e-5, "centroid L2 error {l2}");
    }
}

#[test]
fn incremental_cache_matches_reference_for_days_caps_and_reassignment() {
    let (_directory, vault) = fixture();
    let x = axis(0);
    let y = axis(1);
    let diagonal = vec![0.70710677, 0.70710677]
        .into_iter()
        .chain(std::iter::repeat(0.0).take(algorithm::DIMENSIONS - 2))
        .collect::<Vec<_>>();
    let shared_session = "source:1:100";

    add_atom(
        &vault,
        "atom-a",
        "a.example",
        &x,
        &[
            ("2026-09-30", shared_session, 0.75),
            ("2026-10-01", shared_session, 0.75),
        ],
    );
    assign(&vault, "atom-a", &x);
    let topic_x = assigned_topic(&vault, "atom-a");
    assert_cache_matches_reference(&vault, &topic_x);

    add_atom(
        &vault,
        "atom-b",
        "a.example",
        &x,
        &[
            ("2026-10-01", shared_session, 0.5),
            ("2026-10-01", "source:1:101", 0.5),
        ],
    );
    let expected_b = reference_assignments(&vault.conn, &x);
    assign(&vault, "atom-b", &x);
    assert_assigned_like_reference(&vault, "atom-b", &expected_b);
    assert_eq!(assigned_topic(&vault, "atom-b"), topic_x);
    assert_cache_matches_reference(&vault, &topic_x);

    add_atom(
        &vault,
        "atom-c",
        "b.example",
        &y,
        &[("2026-10-01", "source:1:102", 2.0)],
    );
    assign(&vault, "atom-c", &y);
    let topic_y = assigned_topic(&vault, "atom-c");
    assert_ne!(topic_x, topic_y);
    assert_cache_matches_reference(&vault, &topic_y);

    add_atom(
        &vault,
        "atom-d",
        "a.example",
        &diagonal,
        &[
            ("2026-10-01", shared_session, 0.25),
            ("2026-10-02", "source:1:103", 0.75),
        ],
    );
    let expected_d = reference_assignments(&vault.conn, &diagonal);
    assign(&vault, "atom-d", &diagonal);
    assert_assigned_like_reference(&vault, "atom-d", &expected_d);
    let memberships: Vec<(String, f64)> = vault
        .conn
        .prepare("SELECT topic,mass FROM atom_topics WHERE atom='atom-d' ORDER BY topic")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        memberships.len(),
        2,
        "near tie follows soft-membership rule"
    );
    for topic in [&topic_x, &topic_y] {
        assert_cache_matches_reference(&vault, topic);
    }

    // Reweight/reassign an existing atom to a soft membership and verify both
    // its removed and newly affected topic buckets against the full oracle.
    add_atom(
        &vault,
        "atom-e",
        "c.example",
        &diagonal,
        &[("2026-10-02", "source:1:104", 1.25)],
    );
    let expected_e = reference_assignments(&vault.conn, &diagonal);
    assign(&vault, "atom-e", &diagonal);
    assert_assigned_like_reference(&vault, "atom-e", &expected_e);
    let memberships: Vec<String> = vault
        .conn
        .prepare("SELECT topic FROM atom_topics WHERE atom='atom-e' ORDER BY topic")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(memberships.len(), 2);
    for topic in [&topic_x, &topic_y] {
        assert_cache_matches_reference(&vault, topic);
    }

    // Reweight and move the same atom from a two-topic soft membership to one
    // topic after its site/session contribution changes.
    vault
        .conn
        .execute(
            "UPDATE atoms SET site='moved.example' WHERE id='atom-e'",
            [],
        )
        .unwrap();
    vault
        .conn
        .execute("UPDATE atom_days SET mass=mass+0.5 WHERE atom='atom-e'", [])
        .unwrap();
    vault
        .conn
        .execute(
            "UPDATE vectors SET vector=? WHERE atom='atom-e' AND model='model'",
            [bytes(&x)],
        )
        .unwrap();
    let expected_e_reassigned = reference_assignments_except(&vault.conn, &x, Some("atom-e"));
    assign(&vault, "atom-e", &x);
    assert_assigned_like_reference(&vault, "atom-e", &expected_e_reassigned);
    let moved_topics: Vec<String> = vault
        .conn
        .prepare("SELECT topic FROM atom_topics WHERE atom='atom-e'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(moved_topics, vec![topic_x.clone()]);
    for topic in [&topic_x, &topic_y] {
        if vault
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM topics WHERE id=?)",
                [topic],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        {
            assert_cache_matches_reference(&vault, topic);
        }
    }

    // Updating the authoritative day mass invalidates the cache before the
    // next assignment; a reference rebuild restores exact bucket sufficient
    // statistics before the score can select a topic.
    vault
        .conn
        .execute(
            "UPDATE atom_days SET mass=mass+0.125 WHERE atom='atom-a' AND session=?",
            [shared_session],
        )
        .unwrap();
    let topic = assigned_topic(&vault, "atom-a");
    let valid: i64 = vault
        .conn
        .query_row(
            "SELECT valid FROM topic_cache WHERE topic=?",
            [&topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(valid, 0);
    add_atom(
        &vault,
        "atom-f",
        "a.example",
        &x,
        &[("2026-10-03", shared_session, 1.0)],
    );
    assign(&vault, "atom-f", &x);
    for topic in [&topic_x, &topic_y] {
        if vault
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM topics WHERE id=?)",
                [topic],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        {
            assert_cache_matches_reference(&vault, topic);
        }
    }

    let db_path = vault.path.clone();
    drop(vault);
    let reopened = Vault::open(&db_path).unwrap();
    for topic in [&topic_x, &topic_y] {
        if reopened
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM topics WHERE id=?)",
                [topic],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        {
            assert_cache_matches_reference(&reopened, topic);
        }
    }
}

#[test]
fn accumulator_and_dynamic_type_corruption_rebuild_before_assignment() {
    let (_directory, vault) = fixture();
    let vector = axis(0);
    for (atom, session) in [("atom-a", "source:1:1"), ("atom-b", "source:1:1")] {
        add_atom(
            &vault,
            atom,
            "a.example",
            &vector,
            &[("2026-10-01", session, 1.0)],
        );
        assign(&vault, atom, &vector);
    }
    let topic = assigned_topic(&vault, "atom-a");

    // Same-length accumulator corruption without changing its digest must be
    // rejected before the cached centroid can guide the next decision.
    vault
        .conn
        .execute(
            "UPDATE topic_cache SET sum=zeroblob(?) WHERE topic=?",
            params![algorithm::DIMENSIONS * 8, topic],
        )
        .unwrap();
    add_atom(
        &vault,
        "atom-c",
        "a.example",
        &vector,
        &[("2026-10-02", "source:1:2", 0.5)],
    );
    assign(&vault, "atom-c", &vector);
    assert_eq!(assigned_topic(&vault, "atom-c"), topic);
    assert_cache_matches_reference(&vault, &topic);

    // SQLite's dynamic typing can bypass the cache CHECKs in a damaged or
    // externally edited file. Treat wrong storage classes as invalid cache
    // data and fall back to the authoritative member scan.
    vault
        .conn
        .execute_batch("PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    vault
        .conn
        .execute(
            "UPDATE topic_cache_buckets SET mass='wrong-type' WHERE topic=?",
            [&topic],
        )
        .unwrap();
    vault
        .conn
        .execute("UPDATE topic_cache SET valid=1 WHERE topic=?", [&topic])
        .unwrap();
    add_atom(
        &vault,
        "atom-d",
        "a.example",
        &vector,
        &[("2026-10-03", "source:1:1", 0.25)],
    );
    assign(&vault, "atom-d", &vector);
    assert_eq!(assigned_topic(&vault, "atom-d"), topic);
    assert_cache_matches_reference(&vault, &topic);

    vault
        .conn
        .execute(
            "UPDATE topic_cache SET sum=printf('%2048s','malformed-text') WHERE topic=?",
            [&topic],
        )
        .unwrap();
    add_atom(
        &vault,
        "atom-e",
        "a.example",
        &vector,
        &[("2026-10-04", "source:1:4", 0.75)],
    );
    assign(&vault, "atom-e", &vector);
    assert_eq!(assigned_topic(&vault, "atom-e"), topic);
    assert_cache_matches_reference(&vault, &topic);
}

#[test]
fn zero_mass_bucket_keeps_the_zero_sum_fallback() {
    let (_directory, vault) = fixture();
    let vector = axis(0);
    add_atom(
        &vault,
        "zero-mass",
        "a.example",
        &vector,
        &[("2026-10-01", "source:1:1", 0.0)],
    );
    assign(&vault, "zero-mass", &vector);
    let zero_topic = assigned_topic(&vault, "zero-mass");
    assert_cache_matches_reference(&vault, &zero_topic);
    let zero_buckets: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM topic_cache_buckets WHERE topic=?",
            [&zero_topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(zero_buckets, 0);
    let zero_sum: Vec<u8> = vault
        .conn
        .query_row(
            "SELECT sum FROM topic_cache WHERE topic=?",
            [&zero_topic],
            |row| row.get(0),
        )
        .unwrap();
    assert!(zero_sum
        .chunks_exact(8)
        .all(|chunk| f64::from_le_bytes(chunk.try_into().unwrap()) == 0.0));

    add_atom(
        &vault,
        "positive-mass",
        "a.example",
        &vector,
        &[("2026-10-02", "source:1:2", 1.0)],
    );
    assign(&vault, "positive-mass", &vector);
    assert_ne!(
        assigned_topic(&vault, "positive-mass"),
        zero_topic,
        "the stored initial vector must not replace the zero reference sum"
    );
}

#[test]
fn malformed_or_missing_bucket_rows_invalidate_before_assignment() {
    let (_directory, vault) = fixture();
    let vector = axis(0);
    for (atom, session) in [("atom-a", "source:1:1"), ("atom-b", "source:1:1")] {
        add_atom(
            &vault,
            atom,
            "a.example",
            &vector,
            &[("2026-10-01", session, 1.0)],
        );
        assign(&vault, atom, &vector);
    }
    let topic = assigned_topic(&vault, "atom-a");

    // A missing contribution must invalidate the accumulator and cause a full
    // source-of-truth rebuild before it can guide the next assignment.
    vault
        .conn
        .execute("DELETE FROM topic_cache_buckets WHERE topic=?", [&topic])
        .unwrap();
    let valid: i64 = vault
        .conn
        .query_row(
            "SELECT valid FROM topic_cache WHERE topic=?",
            [&topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(valid, 0);
    add_atom(
        &vault,
        "atom-c",
        "a.example",
        &vector,
        &[("2026-10-02", "source:1:2", 0.5)],
    );
    assign(&vault, "atom-c", &vector);
    assert_cache_matches_reference(&vault, &topic);

    // Moving a bucket key and corrupting a same-length resultant are both
    // observable trigger events. The hash is also checked if the row changed.
    vault
        .conn
        .execute(
            "UPDATE topic_cache_buckets SET session='moved-session' WHERE rowid=(SELECT rowid FROM topic_cache_buckets WHERE topic=? ORDER BY site,session LIMIT 1)",
            [&topic],
        )
        .unwrap();
    assert_eq!(
        vault
            .conn
            .query_row(
                "SELECT valid FROM topic_cache WHERE topic=?",
                [&topic],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    add_atom(
        &vault,
        "atom-d",
        "a.example",
        &vector,
        &[("2026-10-03", "source:1:3", 0.25)],
    );
    assign(&vault, "atom-d", &vector);
    assert_cache_matches_reference(&vault, &topic);

    let blob = vec![0u8; algorithm::DIMENSIONS * 8];
    vault
        .conn
        .execute(
            "UPDATE topic_cache_buckets SET resultant=? WHERE rowid=(SELECT rowid FROM topic_cache_buckets WHERE topic=? ORDER BY site,session LIMIT 1)",
            params![blob, topic],
        )
        .unwrap();
    add_atom(
        &vault,
        "atom-e",
        "a.example",
        &vector,
        &[("2026-10-04", "source:1:4", 0.75)],
    );
    assign(&vault, "atom-e", &vector);
    assert_cache_matches_reference(&vault, &topic);
}

#[test]
fn cap_overflow_uses_reference_then_rebuilds_after_bucket_count_falls() {
    let (_directory, vault) = fixture();
    let vector = axis(0);
    let sessions = (0..=4096)
        .map(|index| ("2026-10-01".to_owned(), format!("source:1:{index}"), 1.0))
        .collect::<Vec<_>>();
    let day_refs = sessions
        .iter()
        .map(|(day, session, mass)| (day.as_str(), session.as_str(), *mass))
        .collect::<Vec<_>>();
    add_atom(&vault, "large-topic", "large.example", &vector, &day_refs);
    assign(&vault, "large-topic", &vector);
    let topic = assigned_topic(&vault, "large-topic");
    let rows: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM topic_cache_buckets WHERE topic=?",
            [&topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "oversized topic must stay uncached");

    vault
        .conn
        .execute(
            "DELETE FROM atom_days WHERE atom='large-topic' AND session='source:1:4096'",
            [],
        )
        .unwrap();
    assign(&vault, "large-topic", &vector);
    let rows: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM topic_cache_buckets WHERE topic=?",
            [&topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 4096,
        "exactly the cache limit should remain cacheable"
    );

    vault
        .conn
        .execute(
            "INSERT INTO atom_days VALUES('large-topic','2026-10-01','source:1:4096',1.0)",
            [],
        )
        .unwrap();
    assign(&vault, "large-topic", &vector);
    let rows: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM topic_cache_buckets WHERE topic=?",
            [&topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 0,
        "the first row above the limit must use the reference path"
    );

    vault.conn.execute(
        "DELETE FROM atom_days WHERE atom='large-topic' AND session IN ('source:1:4095','source:1:4096')",
        [],
    ).unwrap();
    assign(&vault, "large-topic", &vector);
    let rows: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM topic_cache_buckets WHERE topic=?",
            [&topic],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 4095,
        "dropping under the cap should cache all buckets"
    );
    assert_cache_matches_reference(&vault, &topic);
}

fn drop_v5_cache_schema_for_v4(conn: &Connection) {
    conn.execute_batch(
        "DROP TRIGGER topic_cache_atom_days_insert;
         DROP TRIGGER topic_cache_atom_days_update;
         DROP TRIGGER topic_cache_atom_days_delete;
         DROP TRIGGER topic_cache_atom_topics_insert;
         DROP TRIGGER topic_cache_atom_topics_update;
         DROP TRIGGER topic_cache_atom_topics_delete;
         DROP TRIGGER topic_cache_vectors_insert;
         DROP TRIGGER topic_cache_vectors_update;
         DROP TRIGGER topic_cache_vectors_delete;
         DROP TRIGGER topic_cache_feedback_insert;
         DROP TRIGGER topic_cache_feedback_update;
         DROP TRIGGER topic_cache_feedback_delete;
         DROP TRIGGER topic_cache_bucket_insert;
         DROP TRIGGER topic_cache_bucket_update;
         DROP TRIGGER topic_cache_bucket_delete;
         DROP TRIGGER topic_cache_atoms_update;
         DROP TABLE topic_cache_buckets;
         DROP TABLE topic_cache;
         PRAGMA user_version=4;",
    )
    .unwrap();
}

fn row_count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn foreign_key_violations(conn: &Connection) -> Vec<(String, i64, String, i64)> {
    conn.prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn schema_five_is_lazy_and_four_to_five_migration_is_atomic() {
    let (directory, vault) = fixture();
    assert_eq!(
        vault
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(row_count(&vault.conn, "topic_cache"), 0);
    assert_eq!(row_count(&vault.conn, "topic_cache_buckets"), 0);

    let vector = axis(0);
    add_atom(
        &vault,
        "migration-atom",
        "a.example",
        &vector,
        &[("2026-10-01", "source:1:1", 1.0)],
    );
    assign(&vault, "migration-atom", &vector);
    let topics_before = row_count(&vault.conn, "topics");
    let memberships_before = row_count(&vault.conn, "atom_topics");
    assert!(row_count(&vault.conn, "topic_cache") > 0);
    drop_v5_cache_schema_for_v4(&vault.conn);
    drop(vault);

    let upgraded = Vault::open(&directory.path().join("topic-cache.sqlite")).unwrap();
    assert_eq!(row_count(&upgraded.conn, "topics"), topics_before);
    assert_eq!(row_count(&upgraded.conn, "atom_topics"), memberships_before);
    assert_eq!(row_count(&upgraded.conn, "topic_cache"), 0);
    assert_eq!(row_count(&upgraded.conn, "topic_cache_buckets"), 0);
    assert!(foreign_key_violations(&upgraded.conn).is_empty());
    drop(upgraded);

    // Inject a conflicting late trigger into a valid schema-4 layout. The
    // cache table DDL and earlier triggers must all roll back with user_version.
    let raw = Connection::open(directory.path().join("topic-cache.sqlite")).unwrap();
    drop_v5_cache_schema_for_v4(&raw);
    raw.execute_batch(
        "CREATE TRIGGER topic_cache_atom_topics_insert BEFORE INSERT ON atom_topics BEGIN SELECT 1; END;",
    )
    .unwrap();
    drop(raw);
    assert!(Vault::open(&directory.path().join("topic-cache.sqlite")).is_err());
    let rolled_back = Connection::open(directory.path().join("topic-cache.sqlite")).unwrap();
    assert_eq!(
        rolled_back
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(row_count(&rolled_back, "topics"), topics_before);
    assert!(!table_exists(&rolled_back, "topic_cache"));
    assert!(!table_exists(&rolled_back, "topic_cache_buckets"));
    assert!(foreign_key_violations(&rolled_back).is_empty());
    drop(rolled_back);

    let raw = Connection::open(directory.path().join("topic-cache.sqlite")).unwrap();
    raw.execute_batch("DROP TRIGGER topic_cache_atom_topics_insert;")
        .unwrap();
    drop(raw);
    let retried = Vault::open(&directory.path().join("topic-cache.sqlite")).unwrap();
    assert_eq!(
        retried
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert!(foreign_key_violations(&retried.conn).is_empty());
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
        != 0
}

#[test]
fn feedback_dirties_global_label_without_touching_observed_vectors() {
    let (_directory, mut vault) = fixture();
    let vector = axis(0);
    let atom_a = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let atom_b = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    for (atom, title) in [
        (atom_a, "Desk lamp installation guide"),
        (atom_b, "Alhambra chair adjustment guide"),
    ] {
        add_atom(
            &vault,
            atom,
            "a.example",
            &vector,
            &[("2026-10-01", "source:1:1", 1.0)],
        );
        assign(&vault, atom, &vector);
        vault
            .conn
            .execute("UPDATE atoms SET title=? WHERE id=?", params![title, atom])
            .unwrap();
    }
    let topic = assigned_topic(&vault, atom_a);
    let initial = inference::activity(&vault.conn, Some("model")).unwrap();
    assert_eq!(initial[0]["label"], "Desk lamp installation guide");
    let before: Vec<u8> = vault
        .conn
        .query_row("SELECT vector FROM vectors WHERE atom=?", [atom_a], |row| {
            row.get(0)
        })
        .unwrap();

    vault
        .feedback("topic-cache-test", atom_a, "do_not_use", None)
        .unwrap();
    assert_eq!(
        vault
            .conn
            .query_row(
                "SELECT dirty_label FROM topic_cache WHERE topic=?",
                [&topic],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let after = inference::activity(&vault.conn, Some("model")).unwrap();
    assert_eq!(after[0]["label"], "Alhambra chair adjustment guide");
    assert_ne!(after[0]["label"], "Desk lamp installation guide");
    let after_vector: Vec<u8> = vault
        .conn
        .query_row("SELECT vector FROM vectors WHERE atom=?", [atom_a], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        before, after_vector,
        "feedback must not rewrite observed vectors"
    );

    vault
        .conn
        .execute("DELETE FROM atoms WHERE id=?", [atom_b])
        .unwrap();
    assert_eq!(row_count(&vault.conn, "topics"), 0);
    assert_eq!(row_count(&vault.conn, "topic_cache"), 0);
    assert_eq!(row_count(&vault.conn, "topic_cache_buckets"), 0);
}

#[test]
fn confirmed_label_uses_assertion_text_without_original_title_fallback() {
    let (_directory, mut vault) = fixture();
    let vector = axis(0);
    let atom = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    add_atom(
        &vault,
        atom,
        "a.example",
        &vector,
        &[("2026-10-01", "source:1:1", 1.0)],
    );
    assign(&vault, atom, &vector);
    vault
        .conn
        .execute(
            "UPDATE atoms SET title='Old irrelevant search title' WHERE id=?",
            [atom],
        )
        .unwrap();
    vault
        .feedback(
            "topic-cache-test",
            atom,
            "confirm_constraint",
            Some("Use the left mounting bracket for the desk lamp."),
        )
        .unwrap();
    let topics = inference::activity(&vault.conn, Some("model")).unwrap();
    assert_eq!(
        topics[0]["label"],
        "Use the left mounting bracket for the desk lamp."
    );
}

#[allow(dead_code)]
fn _source_root_for_migration_docs() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}
