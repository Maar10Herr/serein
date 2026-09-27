use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::tempdir;

use serein_core::{id, now, storage::Vault, Event, Policy, Recall};

#[derive(Deserialize)]
struct FixtureFile {
    description: String,
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    id: String,
    kind: String,
    language: String,
    observations: Vec<FixtureObservation>,
    query: String,
    query_facets: Vec<String>,
    capture_expectation: String,
    expected_match: ExpectedMatch,
    #[serde(default)]
    privacy_trigger: Option<String>,
}

#[derive(Deserialize)]
struct FixtureObservation {
    id: String,
    title: String,
    search_query: Option<String>,
    site_key: String,
}

#[derive(Deserialize)]
struct ExpectedMatch {
    lexical_baseline_ids: Vec<String>,
    semantic_augmented_ids: Vec<String>,
}

struct Prepared {
    sources: HashMap<String, String>,
    atom_observations: HashMap<String, BTreeSet<String>>,
    privacy_sensitive_cases: HashSet<String>,
    privacy_private_session_cases: HashSet<String>,
    ingest_failures: Vec<String>,
    paused_rejections: Vec<Value>,
    ingested_observation_count: usize,
    unsupported_observation_count: usize,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_path() -> PathBuf {
    repo_root().join("fixtures/retrieval_privacy_cases.json")
}

fn model_pack_path() -> PathBuf {
    repo_root().join("models/pack")
}

fn model_pack_present(path: &Path) -> bool {
    [
        "manifest.json",
        "weights.i8",
        "scales.f32",
        "tokenizer.json",
    ]
    .iter()
    .all(|name| path.join(name).is_file())
}

fn prepare(vault: &mut Vault, cases: &[FixtureCase]) -> Prepared {
    let mut sources = HashMap::new();
    let mut atom_observations: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut privacy_sensitive_cases = HashSet::new();
    let mut privacy_private_session_cases = HashSet::new();
    let mut ingest_failures = Vec::new();
    let mut paused_rejections = Vec::new();
    let mut ingested_observation_count = 0;
    let mut unsupported_observation_count = 0;

    for case in cases {
        let source = id();
        sources.insert(case.id.clone(), source.clone());
        let is_sensitive_placeholder =
            case.privacy_trigger.as_deref() == Some("sensitive_metadata");
        let is_private_session = case.privacy_trigger.as_deref() == Some("private_session");
        let paused = is_private_session;
        if is_sensitive_placeholder {
            privacy_sensitive_cases.insert(case.id.clone());
            unsupported_observation_count += case.observations.len();
        }
        if is_private_session {
            privacy_private_session_cases.insert(case.id.clone());
        }

        let policy = Policy {
            consent: true,
            paused,
            recall_enabled: true,
            selected_only: false,
            selected_sites: vec![],
            excluded_sites: vec![],
            capture_epoch: 1,
        };
        vault
            .set_policy(&source, &policy)
            .expect("synthetic policy is valid");

        // Sensitive titles are deliberately redacted in the shared fixture, so
        // these rows cannot exercise the classifier and are not ingested.
        if is_sensitive_placeholder {
            continue;
        }

        let mut events = Vec::new();
        let mut event_observations = Vec::new();
        for observation in &case.observations {
            let search = observation.search_query.is_some();
            let event_id = id();
            events.push(Event {
                event_id: event_id.clone(),
                visit_id: id(),
                site_key: observation.site_key.clone(),
                site_epoch: 0,
                observed_at: now(),
                kind: if search { "search" } else { "visit" }.into(),
                title: observation.title.clone(),
                search_query: observation.search_query.clone(),
                foreground_seconds: if search { 2 } else { 10 },
            });
            event_observations.push((event_id, observation.id.clone()));
        }
        let ingest = match vault.ingest(&source, 1, events) {
            Ok(value) => value,
            Err(error) => {
                ingest_failures.push(format!("{}: {error}", case.id));
                continue;
            }
        };
        if is_private_session {
            let rejected = ingest["rejected"].as_array().cloned().unwrap_or_default();
            if rejected.len() != event_observations.len()
                || rejected
                    .iter()
                    .any(|item| item["reason"] != "CAPTURE_DISABLED")
            {
                ingest_failures.push(format!(
                    "{}: paused-session records were not all rejected as CAPTURE_DISABLED",
                    case.id
                ));
            }
            paused_rejections.extend(rejected);
            continue;
        }

        let acknowledged = ingest["acknowledged_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if acknowledged.len() != event_observations.len() {
            ingest_failures.push(format!(
                "{}: expected {} acknowledgements, got {}",
                case.id,
                event_observations.len(),
                acknowledged.len()
            ));
        }
        for (event_id, observation_id) in event_observations {
            let atom_id = vault.conn.query_row(
                "SELECT atom FROM events WHERE source=?1 AND id=?2",
                rusqlite::params![source, event_id],
                |row| row.get::<_, String>(0),
            );
            match atom_id {
                Ok(atom_id) => {
                    atom_observations
                        .entry(atom_id)
                        .or_default()
                        .insert(observation_id);
                    ingested_observation_count += 1;
                }
                Err(error) => ingest_failures.push(format!(
                    "{}: event-to-observation mapping failed: {error}",
                    case.id
                )),
            }
        }
    }

    Prepared {
        sources,
        atom_observations,
        privacy_sensitive_cases,
        privacy_private_session_cases,
        ingest_failures,
        paused_rejections,
        ingested_observation_count,
        unsupported_observation_count,
    }
}

fn expected_ids(case: &FixtureCase, semantic: bool) -> BTreeSet<String> {
    let ids = if semantic {
        &case.expected_match.semantic_augmented_ids
    } else {
        &case.expected_match.lexical_baseline_ids
    };
    ids.iter().cloned().collect()
}

fn quality_class(expected: &BTreeSet<String>, returned: &BTreeSet<String>) -> &'static str {
    if expected.is_empty() && returned.is_empty() {
        "correct_empty"
    } else if expected.is_empty() {
        "unexpected_result"
    } else if returned.is_empty() {
        "miss"
    } else if expected == returned {
        "exact_match"
    } else if expected.iter().any(|id| returned.contains(id)) {
        "partial_or_extra"
    } else {
        "no_relevant_result"
    }
}

fn run_mode(
    vault: &mut Vault,
    cases: &[FixtureCase],
    prepared: &Prepared,
    model_path: &Path,
    semantic: bool,
    hard_failures: &mut Vec<String>,
) -> (Value, Vec<(String, BTreeSet<String>)>) {
    let mode_name = if semantic {
        "semantic_pack"
    } else {
        "lexical_missing_pack"
    };
    let mut case_reports = Vec::new();
    let mut returned_by_case = Vec::new();
    let mut elapsed_ms = Vec::new();
    let mut quality_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut expected_observation_count = 0;
    let mut relevant_return_count = 0;
    let mut returned_observation_count = 0;
    let mut no_match_false_positives = 0;
    let mut semantic_paraphrase_relevant_returns = 0;
    let mut hybrid_mode_cases = 0;
    let mut duplicate_return_count = 0;
    let mut unsupported_attribution_count = 0;
    let mut byte_bound_failure_count = 0;
    let mut privacy_drop_return_count = 0;

    for case in cases {
        let expected = expected_ids(case, semantic);
        let unsupported_sensitive = prepared.privacy_sensitive_cases.contains(&case.id);
        if unsupported_sensitive {
            case_reports.push(json!({
                "case_id":case.id,
                "kind":case.kind,
                "language":case.language,
                "evaluation_status":"unsupported_privacy_placeholder",
                "expected_observation_ids":expected,
                "returned_observation_ids":[],
                "note":"Sanitized placeholder contains no classifier trigger; excluded from privacy accuracy scoring."
            }));
            returned_by_case.push((case.id.clone(), BTreeSet::new()));
            continue;
        }

        let source = prepared.sources.get(&case.id).expect("case source exists");
        let request = Recall {
            protocol: 1,
            request_id: id(),
            client: "evaluation-runner".into(),
            vault: "default".into(),
            query: case.query.clone(),
            facets: case.query_facets.clone(),
            scope: vec!["research".into(), "projects".into()],
            max_bytes: 4096,
            budget_ms: 1500,
        };
        let started = Instant::now();
        let response = match vault.recall(&request, source, model_path) {
            Ok(value) => value,
            Err(error) => {
                hard_failures.push(format!(
                    "{} {mode_name}: recall returned error: {error}",
                    case.id
                ));
                case_reports.push(json!({
                    "case_id":case.id,
                    "kind":case.kind,
                    "language":case.language,
                    "evaluation_status":"recall_error",
                    "expected_observation_ids":expected,
                    "returned_observation_ids":[],
                    "error":error.to_string()
                }));
                returned_by_case.push((case.id.clone(), BTreeSet::new()));
                continue;
            }
        };
        let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        elapsed_ms.push(duration_ms);
        let packet_bytes = serde_json::to_vec(&response)
            .expect("recall response serializes")
            .len();
        if packet_bytes > request.max_bytes {
            byte_bound_failure_count += 1;
            hard_failures.push(format!(
                "{} {mode_name}: {} byte packet exceeds {} byte cap",
                case.id, packet_bytes, request.max_bytes
            ));
        }

        let context = response["context"].as_array();
        if context.is_none() {
            hard_failures.push(format!(
                "{} {mode_name}: response has no context array",
                case.id
            ));
        }
        let context = context.cloned().unwrap_or_default();
        let mut atom_ids = Vec::new();
        let mut returned_observations = BTreeSet::new();
        let mut unmapped_atoms = 0;
        for record in &context {
            let atom_id = record["id"].as_str().unwrap_or_default();
            if atom_id.is_empty() {
                hard_failures.push(format!(
                    "{} {mode_name}: returned record has no id",
                    case.id
                ));
            } else {
                atom_ids.push(atom_id.to_string());
                if let Some(observations) = prepared.atom_observations.get(atom_id) {
                    returned_observations.extend(observations.iter().cloned());
                } else {
                    unmapped_atoms += 1;
                }
            }
            if record["state"] != "observed" || record["subject"] != "unknown" {
                unsupported_attribution_count += 1;
                hard_failures.push(format!(
                    "{} {mode_name}: browser evidence attribution was state={:?}, subject={:?}",
                    case.id, record["state"], record["subject"]
                ));
            }
        }
        if unmapped_atoms > 0 {
            hard_failures.push(format!(
                "{} {mode_name}: {unmapped_atoms} returned atom(s) had no fixture mapping",
                case.id
            ));
        }
        let mut seen = HashSet::new();
        let duplicate_ids: Vec<String> = atom_ids
            .iter()
            .filter(|atom| !seen.insert((*atom).clone()))
            .cloned()
            .collect();
        if !duplicate_ids.is_empty() {
            duplicate_return_count += duplicate_ids.len();
            hard_failures.push(format!(
                "{} {mode_name}: duplicate returned atom IDs {:?}",
                case.id, duplicate_ids
            ));
        }

        let actual_mode = response["index"]["mode"].as_str().unwrap_or("unknown");
        if actual_mode == "hybrid" {
            hybrid_mode_cases += 1;
        }
        let status = response["status"].as_str().unwrap_or("unknown");
        let class = quality_class(&expected, &returned_observations);
        *quality_counts.entry(class.into()).or_default() += 1;
        expected_observation_count += expected.len();
        returned_observation_count += returned_observations.len();
        relevant_return_count += expected.intersection(&returned_observations).count();
        if case.kind == "no_match" && !returned_observations.is_empty() {
            no_match_false_positives += 1;
        }
        if semantic
            && case.kind == "semantic_paraphrase"
            && expected.iter().any(|id| returned_observations.contains(id))
        {
            semantic_paraphrase_relevant_returns += 1;
        }
        if case.capture_expectation == "drop" {
            if !returned_observations.is_empty() {
                privacy_drop_return_count += 1;
                hard_failures.push(format!(
                    "{} {mode_name}: privacy-drop evidence was returned",
                    case.id
                ));
            }
            let stored_count: i64 = vault
                .conn
                .query_row(
                    "SELECT count(*) FROM atoms WHERE source=?1",
                    [source],
                    |row| row.get(0),
                )
                .expect("count source atoms");
            if stored_count != 0 {
                hard_failures.push(format!(
                    "{} {mode_name}: privacy-drop source retained {stored_count} atom(s)",
                    case.id
                ));
            }
        }

        let returned_vec: Vec<String> = returned_observations.iter().cloned().collect();
        let expected_vec: Vec<String> = expected.iter().cloned().collect();
        let evaluation_status = if prepared.privacy_private_session_cases.contains(&case.id) {
            "native_paused_session_control"
        } else {
            "evaluated"
        };
        case_reports.push(json!({
            "case_id":case.id,
            "kind":case.kind,
            "language":case.language,
            "evaluation_status":evaluation_status,
            "expected_observation_ids":expected_vec,
            "returned_observation_ids":returned_vec,
            "quality_class":class,
            "returned_atom_count":atom_ids.len(),
            "duplicate_return_ids":duplicate_ids,
            "response_status":status,
            "index_mode":actual_mode,
            "packet_bytes":packet_bytes,
            "max_bytes":request.max_bytes,
            "elapsed_ms":duration_ms,
            "privacy_trigger":case.privacy_trigger,
        }));
        returned_by_case.push((case.id.clone(), returned_observations));
    }

    elapsed_ms.sort_by(|a, b| a.total_cmp(b));
    let p95_ms = if elapsed_ms.is_empty() {
        Value::Null
    } else {
        let index = ((elapsed_ms.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(elapsed_ms.len() - 1);
        json!(elapsed_ms[index])
    };
    let median_ms = match elapsed_ms.len() {
        0 => None,
        n if n % 2 == 0 => Some((elapsed_ms[n / 2 - 1] + elapsed_ms[n / 2]) / 2.0),
        n => Some(elapsed_ms[n / 2]),
    };
    let privacy_case_count =
        prepared.privacy_private_session_cases.len() + prepared.privacy_sensitive_cases.len();
    (
        json!({
            "mode":mode_name,
            "model_path":if mode_name == "lexical" {"missing-pack test fixture"} else {"models/pack"},
            "case_count":cases.len(),
            "evaluated_case_count":case_reports.iter().filter(|c| c["evaluation_status"] == "evaluated" || c["evaluation_status"] == "native_paused_session_control").count(),
            "unsupported_privacy_placeholder_count":prepared.privacy_sensitive_cases.len(),
            "privacy_case_count":privacy_case_count,
            "quality_summary":{
                "classification_counts":quality_counts,
                "expected_observation_count":expected_observation_count,
                "returned_observation_count":returned_observation_count,
                "relevant_return_count":relevant_return_count,
                "no_match_cases_with_results":no_match_false_positives,
                "semantic_paraphrase_cases_with_relevant_return":semantic_paraphrase_relevant_returns,
                "note":"Curated illustrative fixtures are not a validated human benchmark; counts are descriptive, not a field-quality claim."
            },
            "hard_invariants":{
                "browser_evidence_state_and_subject":"observed/unknown",
                "unexpected_attribution_count":unsupported_attribution_count,
                "duplicate_return_id_count":duplicate_return_count,
                "byte_bound_failure_count":byte_bound_failure_count,
                "privacy_drop_return_count":privacy_drop_return_count
            },
            "latency_ms":{
                "median":median_ms,
                "p95":p95_ms
            },
            "hybrid_index_mode_case_count":hybrid_mode_cases,
            "cases":case_reports
        }),
        returned_by_case,
    )
}

#[test]
fn evaluation_fixtures_report_quality_and_assert_hard_invariants() {
    let root = repo_root();
    let fixture_text = fs::read_to_string(fixture_path()).expect("read retrieval fixture file");
    let fixture: FixtureFile =
        serde_json::from_str(&fixture_text).expect("parse retrieval fixtures");
    let temp = tempdir().expect("create temporary evaluation directory");
    let database_path = temp.path().join("evaluation.sqlite");
    let mut vault = Vault::open(&database_path).expect("open temporary evaluation vault");
    let prepared = prepare(&mut vault, &fixture.cases);
    let mut hard_failures = prepared.ingest_failures.clone();

    let missing_pack = temp.path().join("deliberately-missing-model-pack");
    let (lexical, lexical_returns) = run_mode(
        &mut vault,
        &fixture.cases,
        &prepared,
        &missing_pack,
        false,
        &mut hard_failures,
    );
    let pack_path = model_pack_path();
    let pack_present = model_pack_present(&pack_path);
    let (semantic, semantic_returns) = if pack_present {
        let (report, returns) = run_mode(
            &mut vault,
            &fixture.cases,
            &prepared,
            &pack_path,
            true,
            &mut hard_failures,
        );
        (Some(report), Some(returns))
    } else {
        (None, None)
    };

    let mut lexical_map: HashMap<String, BTreeSet<String>> = lexical_returns.into_iter().collect();
    let mut semantic_map: HashMap<String, BTreeSet<String>> =
        semantic_returns.unwrap_or_default().into_iter().collect();
    let mut semantic_gain_cases = 0;
    let mut semantic_paraphrases = 0;
    for case in &fixture.cases {
        if case.kind != "semantic_paraphrase" || !pack_present {
            continue;
        }
        semantic_paraphrases += 1;
        let lexical_ids = lexical_map.remove(&case.id).unwrap_or_default();
        let semantic_ids = semantic_map.remove(&case.id).unwrap_or_default();
        let expected = expected_ids(case, true);
        if expected
            .iter()
            .any(|id| semantic_ids.contains(id) && !lexical_ids.contains(id))
        {
            semantic_gain_cases += 1;
        }
    }

    let paused_session_case_count = prepared.privacy_private_session_cases.len();
    let sensitive_placeholder_case_count = prepared.privacy_sensitive_cases.len();
    if prepared.paused_rejections.len()
        != prepared
            .privacy_private_session_cases
            .iter()
            .map(|case_id| {
                fixture
                    .cases
                    .iter()
                    .find(|case| &case.id == case_id)
                    .map(|case| case.observations.len())
                    .unwrap_or(0)
            })
            .sum::<usize>()
    {
        hard_failures.push(
            "native paused-session rejection count does not match its synthetic input count".into(),
        );
    }

    let result = json!({
        "report_version":1,
        "build_profile":if cfg!(debug_assertions) {"debug"} else {"release"},
        "fixture_provenance":fixture.description,
        "quality_scope":"Curated illustrative synthetic labels; not validated as a human benchmark and not evidence of field reliability.",
        "fixture_case_count":fixture.cases.len(),
        "languages":fixture.cases.iter().map(|case|case.language.clone()).collect::<BTreeSet<_>>(),
        "capture_preparation":{
            "ingested_observation_count":prepared.ingested_observation_count,
            "unsupported_sensitive_placeholder_observation_count":prepared.unsupported_observation_count,
            "privacy_private_session_cases":paused_session_case_count,
            "privacy_private_session_rejected_event_count":prepared.paused_rejections.len(),
            "privacy_private_session_gate":"Policy.paused native control path; not a browser incognito E2E test.",
            "privacy_sensitive_placeholder_cases":sensitive_placeholder_case_count,
            "privacy_sensitive_classifier_status":"unsupported fixture: titles and queries are redacted, so classifier accuracy cannot be measured from these rows."
        },
        "model_pack":{
            "path":"models/pack",
            "files_present":pack_present,
            "semantic_quality_status":if pack_present {"evaluated"} else {"skipped_pack_missing"}
        },
        "modes":{
            "lexical":lexical,
            "semantic":semantic
        },
        "semantic_paraphrase_gain":{
            "eligible_case_count":semantic_paraphrases,
            "cases_with_expected_id_returned_only_in_semantic_run":semantic_gain_cases
        },
        "hard_invariant_failures":hard_failures
    });

    let report_path = root.join("docs/evaluation-results.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&result).expect("serialize evaluation report"),
    )
    .expect("write evaluation report");

    assert!(
        result["hard_invariant_failures"]
            .as_array()
            .is_some_and(|failures| failures.is_empty()),
        "evaluation hard invariants failed; details were written to {}",
        report_path.display()
    );
    eprintln!(
        "evaluation recorded {} fixture cases; semantic pack {}; quality labels remain illustrative",
        fixture.cases.len(),
        if pack_present {
            "evaluated"
        } else {
            "missing/skipped"
        }
    );
}
