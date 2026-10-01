use rusqlite::params;
use serein_core::{
    algorithm, id,
    model::{self, Encoder},
    now,
    storage::Vault,
};
use std::path::{Path, PathBuf};

fn model_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack")
}

fn model_hash(path: &Path) -> String {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("manifest.json")).unwrap()).unwrap();
    manifest["model_hash"].as_str().unwrap().to_owned()
}

fn fixture() -> (tempfile::TempDir, Vault) {
    let directory = tempfile::tempdir().unwrap();
    let vault = Vault::open(&directory.path().join("context.sqlite")).unwrap();
    (directory, vault)
}

fn bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|component| component.to_le_bytes())
        .collect()
}

fn axis(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0; algorithm::DIMENSIONS];
    vector[index] = 1.0;
    vector
}

fn add_atom(
    vault: &Vault,
    title: &str,
    query: Option<&str>,
    vector: Option<(&str, Vec<u8>)>,
) -> String {
    let atom = id();
    let source = "topic-refresh-test";
    vault
        .conn
        .execute(
            "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
             VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![
                atom,
                source,
                "example.com",
                title,
                query,
                "search",
                "2026-10-01T00:00:00Z",
                now(),
                1,
                format!("canonical-{atom}")
            ],
        )
        .unwrap();
    vault
        .conn
        .execute(
            "INSERT INTO atom_days(atom,day,session,mass) VALUES(?,?,?,?)",
            params![atom, "2026-10-01", "topic-refresh-test:1:100", 1.0],
        )
        .unwrap();
    if let Some((model, vector)) = vector {
        vault
            .conn
            .execute(
                "INSERT INTO vectors(atom,model,vector) VALUES(?,?,?)",
                params![atom, model, vector],
            )
            .unwrap();
    }
    atom
}

fn stored_vector(vault: &Vault, atom: &str) -> (String, Vec<u8>) {
    vault
        .conn
        .query_row(
            "SELECT model,vector FROM vectors WHERE atom=?",
            [atom],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn expected_vector(encoder: &Encoder, title: &str, query: &str) -> Vec<f32> {
    let title = encoder.encode(title).unwrap();
    let query = encoder.encode(query).unwrap();
    model::normalize(
        title
            .iter()
            .zip(query)
            .map(|(title, query)| algorithm::TITLE_WEIGHT * title + algorithm::QUERY_WEIGHT * query)
            .collect(),
    )
    .unwrap()
}

fn assert_valid_vector(vault: &Vault, atom: &str, expected_model: &str) -> Vec<u8> {
    let (model, blob) = stored_vector(vault, atom);
    assert_eq!(model, expected_model);
    assert_eq!(blob.len(), algorithm::DIMENSIONS * 4);
    let vector: Vec<_> = blob
        .chunks_exact(4)
        .map(|component| f32::from_le_bytes(component.try_into().unwrap()))
        .collect();
    assert_eq!(vector.len(), algorithm::DIMENSIONS);
    assert!(vector.iter().all(|component| component.is_finite()));
    blob
}

#[test]
fn topic_only_refresh_reuses_vector_without_changing_confirmation_channel() {
    let (_directory, mut vault) = fixture();
    let path = model_path();
    let hash = model_hash(&path);
    let vector = axis(3);
    let atom = add_atom(&vault, "", None, Some((&hash, bytes(&vector))));
    let before = stored_vector(&vault, &atom).1;
    vault
        .feedback(
            "topic-refresh-test",
            &atom,
            "confirm_constraint",
            Some("My desk lamp is under the window."),
        )
        .unwrap();

    // The bundled encoder returns None for empty text. Successful topic progress therefore
    // proves that topic-only work reused the stored observation vector without encoding it.
    let result = vault.refresh(&path, 60_000).unwrap();
    assert_eq!(result["processed"], 1);
    assert_eq!(result["pending_atoms"], 0);
    assert_eq!(stored_vector(&vault, &atom).1, before);
    let memberships: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM atom_topics WHERE atom=?",
            [&atom],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(memberships, 1);
}

#[test]
fn missing_wrong_model_and_invalid_vectors_are_rebuilt_from_observed_text() {
    let (_directory, mut vault) = fixture();
    let path = model_path();
    let hash = model_hash(&path);
    vault
        .conn
        .execute(
            "INSERT INTO meta(key,value) VALUES('active_model_hash',?)",
            [&hash],
        )
        .unwrap();

    let title = "Desk lamp installation guide";
    let query = "Desk lamp installation";
    let missing = add_atom(&vault, title, Some(query), None);
    let wrong_model = add_atom(
        &vault,
        title,
        Some(query),
        Some(("old-model", bytes(&axis(1)))),
    );
    let short_blob = add_atom(
        &vault,
        title,
        Some(query),
        Some((
            &hash,
            bytes(&axis(5))[..algorithm::DIMENSIONS * 4 - 1].to_vec(),
        )),
    );

    // These same-length corrupt vectors already have current-model topic-skip readiness. They
    // must still be discovered as vector work and regenerated.
    let mut nan_vector = axis(2);
    nan_vector[0] = f32::NAN;
    let nan = add_atom(
        &vault,
        title,
        Some(query),
        Some((&hash, bytes(&nan_vector))),
    );
    let mut infinity_vector = axis(4);
    infinity_vector[1] = f32::INFINITY;
    let infinity = add_atom(
        &vault,
        title,
        Some(query),
        Some((&hash, bytes(&infinity_vector))),
    );
    for atom in [&nan, &infinity, &short_blob] {
        vault
            .conn
            .execute(
                "INSERT INTO topic_skips(atom,model) VALUES(?,?)",
                params![atom, hash],
            )
            .unwrap();
    }

    // A status-only zero-budget pass cannot repair anything, but still must count invalid
    // same-model vectors even when their topic-skip rows otherwise mark topic work complete.
    let pending = vault.refresh(&path, 0).unwrap();
    assert_eq!(pending["processed"], 0);
    assert_eq!(pending["pending_atoms"], 5);

    let encoder = Encoder::open(&path).unwrap();
    let expected = bytes(&expected_vector(&encoder, title, query));
    let result = vault.refresh(&path, 60_000).unwrap();
    assert_eq!(result["processed"], 5);
    assert_eq!(result["pending_atoms"], 0);
    for atom in [&missing, &wrong_model, &nan, &infinity, &short_blob] {
        assert_eq!(assert_valid_vector(&vault, atom, &hash), expected);
    }

    let before_second_refresh: Vec<_> = [&missing, &wrong_model, &nan, &infinity, &short_blob]
        .iter()
        .map(|atom| stored_vector(&vault, atom).1)
        .collect();
    let repeated = vault.refresh(&path, 60_000).unwrap();
    assert_eq!(repeated["processed"], 0);
    assert_eq!(repeated["pending_atoms"], 0);
    let after_second_refresh: Vec<_> = [&missing, &wrong_model, &nan, &infinity, &short_blob]
        .iter()
        .map(|atom| stored_vector(&vault, atom).1)
        .collect();
    assert_eq!(after_second_refresh, before_second_refresh);
}

#[test]
fn model_rotation_discards_incompatible_topics_and_vectors() {
    let (_directory, mut vault) = fixture();
    let path = model_path();
    let hash = model_hash(&path);
    let vector = axis(0);
    let atom = add_atom(
        &vault,
        "Desk lamp installation guide",
        Some("Desk lamp installation"),
        Some(("old-model", bytes(&vector))),
    );
    vault
        .conn
        .execute(
            "INSERT INTO meta(key,value) VALUES('active_model_hash','old-model')",
            [],
        )
        .unwrap();
    vault
        .conn
        .execute(
            "INSERT INTO topics(id,label,centroid,model) VALUES(?,?,?,?)",
            params!["old-topic", "Old label", bytes(&vector), "old-model"],
        )
        .unwrap();
    vault
        .conn
        .execute(
            "INSERT INTO atom_topics(atom,topic,mass) VALUES(?,?,?)",
            params![atom, "old-topic", 1.0],
        )
        .unwrap();

    let result = vault.refresh(&path, 60_000).unwrap();
    assert_eq!(result["processed"], 1);
    assert_eq!(result["pending_atoms"], 0);
    assert_valid_vector(&vault, &atom, &hash);
    let old_topics: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM topics WHERE model='old-model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_topics, 0);
}
