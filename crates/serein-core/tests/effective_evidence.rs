use rusqlite::params;
use serde_json::{json, Value};
use serein_core::{algorithm, id, model, now, storage::Vault, Policy, Recall};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

const SOURCE_A: &str = "11111111-1111-4111-8111-111111111111";
const SOURCE_B: &str = "22222222-2222-4222-8222-222222222222";

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

fn request(query: &str, scope: &[&str], max_bytes: usize) -> Recall {
    Recall {
        protocol: 1,
        request_id: id(),
        client: "effective-evidence-test".into(),
        vault: "default".into(),
        query: query.into(),
        facets: vec![],
        scope: scope.iter().map(|value| (*value).to_owned()).collect(),
        max_bytes,
        budget_ms: 2000,
    }
}

fn lock_data_dir() -> std::sync::MutexGuard<'static, ()> {
    DATA_DIR_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

fn add_observation(
    vault: &Vault,
    atom_id: &str,
    source: &str,
    site: &str,
    title: &str,
    query: Option<&str>,
) {
    let timestamp = now();
    vault
        .conn
        .execute(
            "INSERT INTO atoms(id,source,site,title,query,kind,first_seen,last_seen,seconds,canonical)
             VALUES(?1,?2,?3,?4,?5,'search',?6,?6,20,?7)",
            params![
                atom_id,
                source,
                site,
                title,
                query,
                timestamp,
                format!("effective-evidence:{atom_id}")
            ],
        )
        .unwrap();
    let parsed = chrono::DateTime::parse_from_rfc3339(&timestamp).unwrap();
    vault
        .conn
        .execute(
            "INSERT INTO atom_days(atom,day,session,mass) VALUES(?1,?2,?3,1.0)",
            params![
                atom_id,
                parsed.format("%Y-%m-%d").to_string(),
                format!("{source}:1:{}", parsed.timestamp() / 1800)
            ],
        )
        .unwrap();
}

fn policy_with_exclusion(site: &str) -> Policy {
    Policy {
        excluded_sites: vec![site.to_owned()],
        ..enabled_policy()
    }
}

fn copy_model_pack(destination: &Path) -> PathBuf {
    fs::create_dir_all(destination).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
    destination.to_path_buf()
}

fn copy_and_rotate_model_pack(root: &Path, current: &Path) -> PathBuf {
    let previous_hash = model::current_hash(current);
    let alternate = copy_model_pack(&root.join("models/rotated"));
    let manifest_path = alternate.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["model_hash"] =
        json!("sentence-transformers/static-similarity-mrl-multilingual-v1@test-generation-2");
    let manifest = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(&manifest_path, &manifest).unwrap();
    fs::write(current.join("manifest.json"), manifest).unwrap();
    assert_ne!(
        previous_hash.as_deref(),
        model::current_hash(current).as_deref()
    );
    assert_eq!(
        model::current_hash(current).as_deref(),
        model::current_hash(&alternate).as_deref()
    );
    alternate
}

fn store_vector(vault: &Vault, atom: &str, model_hash: &str, vector: &[f32]) {
    let bytes: Vec<u8> = vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    vault
        .conn
        .execute(
            "INSERT OR REPLACE INTO vectors(atom,model,vector) VALUES(?1,?2,?3)",
            params![atom, model_hash, bytes],
        )
        .unwrap();
}

fn raw_lexical_order(vault: &Vault) -> Vec<String> {
    vault
        .conn
        .prepare(
            "SELECT a.id FROM atom_fts JOIN atoms a ON a.id=atom_fts.id
             WHERE atom_fts MATCH ?1 ORDER BY bm25(atom_fts),a.id",
        )
        .unwrap()
        .query_map(["\"desk\" OR \"lamp\""], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn raw_lexical_top_64(vault: &Vault) -> Vec<String> {
    raw_lexical_order(vault)
        .into_iter()
        .take(algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL)
        .collect()
}

fn raw_semantic_order(vault: &Vault, model_hash: &str, query: &[f32]) -> Vec<String> {
    let rows: Vec<(String, Vec<u8>)> = vault
        .conn
        .prepare("SELECT atom,vector FROM vectors WHERE model=?1")
        .unwrap()
        .query_map([model_hash], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut scored: Vec<_> = rows
        .into_iter()
        .map(|(atom, bytes)| {
            let vector: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|part| f32::from_le_bytes(part.try_into().unwrap()))
                .collect();
            (atom, model::cosine(query, &vector))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(atom, _)| atom).collect()
}

fn raw_semantic_top_64(vault: &Vault, model_hash: &str, query: &[f32]) -> Vec<String> {
    raw_semantic_order(vault, model_hash, query)
        .into_iter()
        .take(algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL)
        .collect()
}

fn context_ids(packet: &Value) -> Vec<String> {
    packet["context"]
        .as_array()
        .expect("recall context array")
        .iter()
        .map(|item| item["id"].as_str().expect("record ID").to_owned())
        .collect()
}

fn empty_context(packet: &Value, explanation: &str) {
    assert!(
        packet["context"].as_array().unwrap().is_empty(),
        "{explanation}: {}",
        packet["context"]
    );
}

fn assert_packet_caps(
    packet: &Value,
    sites_by_id: &std::collections::HashMap<String, String>,
    max_bytes: usize,
) {
    let ids = context_ids(packet);
    assert!(ids.len() <= algorithm::MAX_CONTEXT_RECORDS);
    let mut per_site = std::collections::HashMap::<&str, usize>::new();
    for atom in &ids {
        *per_site
            .entry(
                sites_by_id
                    .get(atom)
                    .expect("only fixture IDs are returned"),
            )
            .or_default() += 1;
    }
    assert!(per_site
        .values()
        .all(|count| *count <= algorithm::MAX_RECORDS_PER_SITE as usize));
    assert!(serde_json::to_vec(packet).unwrap().len() <= max_bytes);
}

#[test]
fn r04_disallowed_records_do_not_starve_the_64_slot_lexical_or_hybrid_channels() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let model_path = copy_model_pack(&temp.path().join("models/current"));
    let model_hash = model::current_hash(&model_path).unwrap();
    let mut vault = Vault::open(&temp.path().join("r04.sqlite")).unwrap();
    vault
        .set_policy(SOURCE_A, &policy_with_exclusion("hidden.example.org"))
        .unwrap();

    let repeated_desk_lamp = "Desk lamp ".repeat(12);
    let guide_id = fixture_id(9000);
    add_observation(
        &vault,
        &guide_id,
        SOURCE_A,
        "allowed.example.org",
        "Desk lamp installation guide",
        Some("Desk lamp"),
    );

    let mut disallowed_ids = Vec::new();
    for index in 0..66u32 {
        let atom = fixture_id(100 + index);
        let title = format!("{repeated_desk_lamp} fixture record {index:02}");
        add_observation(
            &vault,
            &atom,
            SOURCE_A,
            "allowed.example.org",
            &title,
            Some("Desk lamp"),
        );
        let action = match index {
            0..=21 => "do_not_use",
            22..=43 => "wrong_topic",
            _ => "confirm_constraint",
        };
        vault
            .feedback(
                SOURCE_A,
                &atom,
                action,
                (action == "confirm_constraint").then_some("Desk lamp"),
            )
            .unwrap();
        disallowed_ids.push(atom);
    }

    let hidden_id = fixture_id(8000);
    add_observation(
        &vault,
        &hidden_id,
        SOURCE_A,
        "hidden.example.org",
        &format!("{repeated_desk_lamp} excluded control"),
        Some("Desk lamp"),
    );

    // Six eligible records make the final record/site limits observable. The
    // target shares its site with two records, so at most one of those peers
    // can accompany it in the six-record packet.
    let mut eligible_sites = std::collections::HashMap::new();
    eligible_sites.insert(guide_id.clone(), "allowed.example.org".to_owned());
    for index in 0..6u32 {
        let atom = fixture_id(200 + index);
        let site = if index < 2 {
            "allowed.example.org".to_owned()
        } else {
            format!("room-{index}.example.org")
        };
        let filler = " planning reference materials".repeat(8);
        let title = format!("Desk lamp support option {index}{filler}");
        let query = format!("Desk lamp support {index}");
        add_observation(&vault, &atom, SOURCE_A, &site, &title, Some(&query));
        eligible_sites.insert(atom, site);
    }

    // Make the capacity pressure deterministic for the real 256-dimensional
    // model: every forbidden record and the useful guide has exact main-query
    // support, while other eligible records cannot crowd it out semantically.
    let prewarm = vault.refresh(&model_path, 120_000).unwrap();
    assert_eq!(prewarm["pending_atoms"], 0, "model fixture fully indexed");
    let encoder = model::Encoder::open(&model_path).unwrap();
    let query_vector = encoder.encode("Desk lamp").unwrap();
    let mut opposite = query_vector.clone();
    opposite.iter_mut().for_each(|value| *value = -*value);
    for atom in disallowed_ids.iter().chain(std::iter::once(&hidden_id)) {
        store_vector(&vault, atom, &model_hash, &query_vector);
    }
    store_vector(&vault, &guide_id, &model_hash, &query_vector);
    for atom in eligible_sites.keys().filter(|atom| *atom != &guide_id) {
        store_vector(&vault, atom, &model_hash, &opposite);
    }

    // Verify that this deterministic fixture really puts the useful guide
    // behind each unfiltered top-64 channel boundary.
    let lexical_order = raw_lexical_order(&vault);
    let guide_lexical_rank = lexical_order
        .iter()
        .position(|atom| atom == &guide_id)
        .unwrap();
    assert!(guide_lexical_rank >= 64);
    assert!(disallowed_ids.iter().all(|atom| {
        lexical_order
            .iter()
            .position(|ranked| ranked == atom)
            .unwrap()
            < guide_lexical_rank
    }));
    assert!(eligible_sites
        .keys()
        .filter(|atom| *atom != &guide_id)
        .all(|atom| {
            lexical_order
                .iter()
                .position(|ranked| ranked == atom)
                .unwrap()
                > guide_lexical_rank
        }));
    let lexical_top = raw_lexical_top_64(&vault);
    assert_eq!(lexical_top.len(), 64);
    assert!(!lexical_top.contains(&guide_id));
    assert!(lexical_top
        .iter()
        .all(|atom| disallowed_ids.contains(atom) || atom == &hidden_id));
    let semantic_order = raw_semantic_order(&vault, &model_hash, &query_vector);
    let guide_semantic_rank = semantic_order
        .iter()
        .position(|atom| atom == &guide_id)
        .unwrap();
    assert!(guide_semantic_rank >= 64);
    assert!(disallowed_ids.iter().all(|atom| {
        semantic_order
            .iter()
            .position(|ranked| ranked == atom)
            .unwrap()
            < guide_semantic_rank
    }));
    let semantic_top = raw_semantic_top_64(&vault, &model_hash, &query_vector);
    assert_eq!(semantic_top.len(), 64);
    assert!(!semantic_top.contains(&guide_id));
    assert!(semantic_top
        .iter()
        .all(|atom| disallowed_ids.contains(atom) || atom == &hidden_id));

    let lexical = vault
        .recall(
            &request("Desk lamp", &["research"], 16_384),
            SOURCE_A,
            &temp.path().join("model-unavailable"),
        )
        .unwrap();
    assert_eq!(lexical["index"]["mode"], "lexical");
    let lexical_ids = context_ids(&lexical);
    assert!(lexical_ids.contains(&guide_id));
    assert!(!lexical_ids
        .iter()
        .any(|atom| { disallowed_ids.contains(atom) || atom == &hidden_id }));
    assert_eq!(
        lexical_ids.len(),
        algorithm::MAX_CONTEXT_RECORDS,
        "lexical packet IDs: {lexical_ids:?}"
    );
    assert_packet_caps(&lexical, &eligible_sites, 16_384);

    let hybrid = vault
        .recall(
            &request("Desk lamp", &["research"], 16_384),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    assert_eq!(hybrid["index"]["mode"], "hybrid");
    let hybrid_ids = context_ids(&hybrid);
    assert!(hybrid_ids.contains(&guide_id));
    assert!(!hybrid_ids
        .iter()
        .any(|atom| disallowed_ids.contains(atom) || atom == &hidden_id));
    assert_eq!(
        hybrid_ids.len(),
        algorithm::MAX_CONTEXT_RECORDS,
        "hybrid packet IDs: {hybrid_ids:?}"
    );
    assert_packet_caps(&hybrid, &eligible_sites, 16_384);
}

#[test]
fn r05_only_the_latest_confirmation_text_can_support_confirmed_recall() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let model_path = copy_model_pack(&temp.path().join("models/current"));
    let mut vault = Vault::open(&temp.path().join("r05.sqlite")).unwrap();
    vault.set_policy(SOURCE_A, &enabled_policy()).unwrap();

    let telescope_id = fixture_id(7000);
    add_observation(
        &vault,
        &telescope_id,
        SOURCE_A,
        "optics.example.org",
        "Telescope mirror collimation guide",
        Some("Telescope mirror collimation guide"),
    );
    let ferry_id = fixture_id(7001);
    add_observation(
        &vault,
        &ferry_id,
        SOURCE_A,
        "transit.example.org",
        "Ferry schedule and terminal guide",
        Some("Ferry schedule terminal"),
    );
    let prewarm = vault.refresh(&model_path, 60_000).unwrap();
    assert_eq!(prewarm["pending_atoms"], 0);

    vault
        .feedback(
            SOURCE_A,
            &telescope_id,
            "confirm_constraint",
            Some("Telescope primary mirror uses a forty two millimeter aperture."),
        )
        .unwrap();
    vault
        .feedback(
            SOURCE_A,
            &telescope_id,
            "confirm_constraint",
            Some("My office desk must be made of oak."),
        )
        .unwrap();

    let telescope = vault
        .recall(
            &request(
                "Telescope mirror collimation guide",
                &["confirmed_preferences"],
                4096,
            ),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    empty_context(
        &telescope,
        "the current oak confirmation must not inherit telescope title, query, old confirmation, or vector support",
    );
    let mixed_scope_telescope = vault
        .recall(
            &request(
                "Telescope mirror collimation guide",
                &["research", "confirmed_preferences"],
                4096,
            ),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    empty_context(
        &mixed_scope_telescope,
        "including both scopes cannot route an observation's old telescope text after confirmation",
    );

    let ferry = vault
        .recall(
            &request("Ferry schedule terminal", &["confirmed_preferences"], 4096),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    empty_context(
        &ferry,
        "an observed ferry record is not eligible for confirmed-preference scope",
    );

    let office = vault
        .recall(
            &request("My office desk", &["confirmed_preferences"], 4096),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    let office_record = office["context"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == telescope_id)
        .expect("latest confirmation matches the office-desk query");
    assert_eq!(office_record["state"], "confirmed");
    assert_eq!(office_record["text"], "My office desk must be made of oak.");
    assert_eq!(office["context"].as_array().unwrap().len(), 1);

    let research = vault
        .recall(
            &request("My office desk", &["research"], 4096),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    empty_context(
        &research,
        "confirmed evidence is excluded from research-only scope",
    );
}

#[test]
fn r06_supersession_suppression_deletion_and_model_change_never_restore_old_support() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let model_path = copy_model_pack(&temp.path().join("models/current"));
    let mut vault = Vault::open(&temp.path().join("r06.sqlite")).unwrap();
    vault.set_policy(SOURCE_A, &enabled_policy()).unwrap();

    let atom = fixture_id(7100);
    add_observation(
        &vault,
        &atom,
        SOURCE_A,
        "optics.example.org",
        "Telescope mirror collimation guide",
        Some("Telescope mirror collimation guide"),
    );
    let prewarm = vault.refresh(&model_path, 60_000).unwrap();
    assert_eq!(prewarm["pending_atoms"], 0);
    vault
        .feedback(
            SOURCE_A,
            &atom,
            "confirm_constraint",
            Some("Telescope mirror supports a forty two millimeter aperture."),
        )
        .unwrap();
    vault
        .feedback(
            SOURCE_A,
            &atom,
            "confirm_constraint",
            Some("My office desk must be made of oak."),
        )
        .unwrap();

    let old_support = vault
        .recall(
            &request("Telescope mirror", &["confirmed_preferences"], 4096),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    empty_context(
        &old_support,
        "superseded confirmation has no effective support",
    );

    let old_hash = model::current_hash(&model_path).unwrap();
    let alternate = copy_and_rotate_model_pack(temp.path(), &model_path);
    let new_hash = model::current_hash(&alternate).unwrap();
    assert_ne!(old_hash, new_hash);
    let reindexed = vault.refresh(&alternate, 60_000).unwrap();
    assert_eq!(reindexed["pending_atoms"], 0, "rotated model fully indexed");

    let after_model_change = vault
        .recall(
            &request("Telescope mirror", &["confirmed_preferences"], 4096),
            SOURCE_A,
            &alternate,
        )
        .unwrap();
    empty_context(
        &after_model_change,
        "re-encoding the original observation under a new model cannot restore superseded text support",
    );
    let current = vault
        .recall(
            &request("My office desk", &["confirmed_preferences"], 4096),
            SOURCE_A,
            &alternate,
        )
        .unwrap();
    assert_eq!(current["context"][0]["id"], atom);
    assert_eq!(
        current["context"][0]["text"],
        "My office desk must be made of oak."
    );

    vault
        .feedback(SOURCE_A, &atom, "wrong_topic", None)
        .unwrap();
    let suppressed = vault
        .recall(
            &request("My office desk", &["confirmed_preferences"], 4096),
            SOURCE_A,
            &alternate,
        )
        .unwrap();
    empty_context(
        &suppressed,
        "later wrong-topic suppression remains authoritative over the latest confirmation",
    );

    vault.forget(SOURCE_A, None, Some(&atom), 2).unwrap();
    let row_counts: (i64, i64, i64) = vault
        .conn
        .query_row(
            "SELECT
               (SELECT count(*) FROM atoms WHERE id=?1),
               (SELECT count(*) FROM feedback WHERE atom=?1),
               (SELECT count(*) FROM vectors WHERE atom=?1)",
            [&atom],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        row_counts,
        (0, 0, 0),
        "forget cascades old evidence and vectors"
    );
    for query in ["Telescope mirror", "My office desk"] {
        let after_delete = vault
            .recall(
                &request(query, &["confirmed_preferences"], 4096),
                SOURCE_A,
                &alternate,
            )
            .unwrap();
        empty_context(
            &after_delete,
            "deleted evidence cannot support old or latest text",
        );
    }
}

#[test]
fn r06_confirmation_required_questions_privacy_source_and_byte_bounds_still_hold() {
    let _lock = lock_data_dir();
    let temp = tempfile::tempdir().unwrap();
    let _data_dir = DataDirGuard::set(temp.path());
    let model_path = copy_model_pack(&temp.path().join("models/current"));
    let mut vault = Vault::open(&temp.path().join("r06-guards.sqlite")).unwrap();
    vault.set_policy(SOURCE_A, &enabled_policy()).unwrap();
    vault.set_policy(SOURCE_B, &enabled_policy()).unwrap();

    let atom = fixture_id(7200);
    add_observation(
        &vault,
        &atom,
        SOURCE_A,
        "furniture.example.org",
        "Blue chair purchase and model ideas",
        Some("Blue chair purchase options"),
    );
    let prewarm = vault.refresh(&model_path, 60_000).unwrap();
    assert_eq!(prewarm["pending_atoms"], 0);
    let encoder = model::Encoder::open(&model_path).unwrap();
    let question_vector = encoder.encode("Did I buy the blue chair?").unwrap();
    let model_hash = encoder.manifest.model_hash.clone();
    store_vector(&vault, &atom, &model_hash, &question_vector);

    let question = "Did I buy the blue chair?";
    let lexical_question = vault
        .recall(
            &request(question, &["research", "confirmed_preferences"], 4096),
            SOURCE_A,
            &temp.path().join("model-unavailable"),
        )
        .unwrap();
    empty_context(
        &lexical_question,
        "observed activity cannot answer a confirmation-required purchase question",
    );
    let hybrid_question = vault
        .recall(
            &request(question, &["research", "confirmed_preferences"], 4096),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    assert_eq!(hybrid_question["index"]["mode"], "hybrid");
    empty_context(
        &hybrid_question,
        "semantic admission cannot make an unconfirmed observation answer a purchase question",
    );

    let bounded = vault
        .recall(
            &request("Blue chair", &["research"], 1024),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    assert_eq!(bounded["context"][0]["id"], atom);
    assert!(bounded["context"].as_array().unwrap().len() <= algorithm::MAX_CONTEXT_RECORDS);
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= 1024);

    let other_source = vault
        .recall(
            &request("Blue chair", &["research"], 1024),
            SOURCE_B,
            &model_path,
        )
        .unwrap();
    empty_context(
        &other_source,
        "another source cannot see this source's observation",
    );

    let mut private = vault.policy(SOURCE_A).unwrap();
    private.recall_enabled = false;
    private.capture_epoch += 1;
    vault.set_policy(SOURCE_A, &private).unwrap();
    let blocked = vault
        .recall(
            &request("Blue chair", &["research"], 1024),
            SOURCE_A,
            &model_path,
        )
        .unwrap();
    assert_eq!(blocked["status"], "blocked");
    empty_context(&blocked, "disabled recall returns no evidence");
}
