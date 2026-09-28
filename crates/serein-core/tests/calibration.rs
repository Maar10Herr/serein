//! Small, repeatable development calibration runner for the synthetic relevance fixture.
//!
//! Run with:
//! `source .toolchain/env.sh && SEREIN_CALIBRATION_REPORT=$TMPDIR/serein-calibration.json cargo test -p serein-core --test calibration -- --nocapture`
//! Set `SEREIN_CALIBRATION_FIXTURE` to either development fixture, or
//! `SEREIN_CALIBRATION_MODEL_PACK` to evaluate another compatible model pack.
//! A held-out fixture is opt-in with `SEREIN_CALIBRATION_HOLDOUT=1` and is
//! reported without the development regression floor.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use serein_core::{id, policy, storage::Vault, Event, Policy, Recall};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::tempdir;

const CONTEXT_K: usize = 6;
const PACKET_LIMIT: usize = 4096;
const RECALL_BUDGET_MS: u64 = 1500;

#[derive(Debug, Deserialize)]
struct Fixture {
    fixture_id: String,
    evidence: Vec<FixtureEvidence>,
    queries: Vec<FixtureQuery>,
}

#[derive(Debug, Deserialize)]
struct FixtureEvidence {
    id: String,
    title: String,
    hostname: String,
    kind: String,
    dwell_seconds: u32,
    observed_at: String,
    #[serde(default)]
    search_query: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureQuery {
    id: String,
    text: String,
    facets: Vec<String>,
    expected_ids: Vec<String>,
    expected_empty: bool,
}

#[derive(Debug, Deserialize)]
struct HoldoutFixture {
    fixture_id: String,
    observations: Vec<HoldoutObservation>,
    queries: Vec<HoldoutQuery>,
}

#[derive(Debug, Deserialize)]
struct HoldoutObservation {
    id: String,
    evidence: HoldoutEvidence,
}

#[derive(Debug, Deserialize)]
struct HoldoutEvidence {
    title: String,
    host: String,
    query: Option<String>,
    dwell_seconds: u32,
    observed_at: String,
}

#[derive(Debug, Deserialize)]
struct HoldoutQuery {
    id: String,
    question: String,
    facets: Vec<String>,
    relevant_observation_ids: Vec<String>,
    expected_empty: bool,
}

fn read_fixture() -> Fixture {
    let path = fixture_path();
    let bytes = fs::read(&path).expect("read synthetic calibration fixture");
    if std::env::var("SEREIN_CALIBRATION_HOLDOUT").as_deref() == Ok("1")
        && path.file_name().and_then(|name| name.to_str())
            == Some("calibration_holdout_relevance_noise.json")
    {
        let holdout: HoldoutFixture =
            serde_json::from_slice(&bytes).expect("parse held-out synthetic fixture");
        return Fixture {
            fixture_id: holdout.fixture_id,
            evidence: holdout
                .observations
                .into_iter()
                .map(|item| FixtureEvidence {
                    id: item.id,
                    title: item.evidence.title,
                    hostname: item.evidence.host,
                    kind: "visit".to_owned(),
                    dwell_seconds: item.evidence.dwell_seconds,
                    observed_at: item.evidence.observed_at,
                    search_query: item.evidence.query.filter(|query| !query.is_empty()),
                })
                .collect(),
            queries: holdout
                .queries
                .into_iter()
                .map(|query| FixtureQuery {
                    id: query.id,
                    text: query.question,
                    facets: query.facets,
                    expected_ids: query.relevant_observation_ids,
                    expected_empty: query.expected_empty,
                })
                .collect(),
        };
    }
    serde_json::from_slice(&bytes).expect("parse synthetic development fixture")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_path() -> PathBuf {
    std::env::var_os("SEREIN_CALIBRATION_FIXTURE")
        .map(|value| {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                repo_root().join(path)
            }
        })
        .unwrap_or_else(|| repo_root().join("tests/fixtures/calibration_dev_relevance_noise.json"))
}

fn model_pack_path() -> PathBuf {
    std::env::var_os("SEREIN_CALIBRATION_MODEL_PACK")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("models/pack"))
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

fn normalized_timestamps(evidence: &[FixtureEvidence]) -> HashMap<String, String> {
    let latest = evidence
        .iter()
        .map(|item| {
            DateTime::parse_from_rfc3339(&item.observed_at)
                .expect("fixture observation timestamp is RFC 3339")
                .with_timezone(&Utc)
        })
        .max()
        .expect("fixture has evidence");
    // Keep the fixture's relative chronology and session gaps while ensuring this
    // static synthetic dataset remains within the core's 90-day ingest window.
    let shift = Utc::now() - Duration::minutes(10) - latest;
    evidence
        .iter()
        .map(|item| {
            let timestamp = DateTime::parse_from_rfc3339(&item.observed_at)
                .expect("fixture observation timestamp is RFC 3339")
                .with_timezone(&Utc)
                + shift;
            (
                item.id.clone(),
                timestamp.to_rfc3339_opts(SecondsFormat::Secs, true),
            )
        })
        .collect()
}

fn make_event(item: &FixtureEvidence, observed_at: &str) -> Event {
    let is_search = item.kind == "search";
    Event {
        event_id: id(),
        visit_id: id(),
        site_key: item.hostname.clone(),
        site_epoch: 0,
        observed_at: observed_at.to_owned(),
        kind: if is_search { "search" } else { "visit" }.to_owned(),
        title: item.title.clone(),
        search_query: item
            .search_query
            .clone()
            .or_else(|| is_search.then(|| item.title.clone())),
        foreground_seconds: item.dwell_seconds.min(3600),
    }
}

fn atom_for_evidence(
    vault: &Vault,
    source: &str,
    item: &FixtureEvidence,
) -> rusqlite::Result<String> {
    let kind = if item.kind == "search" {
        "search"
    } else {
        "visit"
    };
    let title = policy::clean(&item.title, 256);
    let query = item
        .search_query
        .as_deref()
        .or_else(|| (kind == "search").then_some(item.title.as_str()))
        .map(|query| policy::clean(query, 512));
    vault.conn.query_row(
        "SELECT id FROM atoms WHERE source=?1 AND site=?2 AND kind=?3 AND title=?4 AND query IS ?5 LIMIT 1",
        rusqlite::params![source, item.hostname, kind, title, query],
        |row| row.get(0),
    )
}

fn average(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn percentile(values: &[f64], percentile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    Some(sorted[index])
}

fn dcg(relevances: impl IntoIterator<Item = bool>) -> f64 {
    relevances
        .into_iter()
        .take(CONTEXT_K)
        .enumerate()
        .filter_map(|(index, relevant)| relevant.then(|| 1.0 / ((index + 2) as f64).log2()))
        .sum()
}

fn write_report(report: &Value) -> Option<PathBuf> {
    let path = std::env::var_os("SEREIN_CALIBRATION_REPORT").map(PathBuf::from)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).expect("create calibration report directory");
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(report).expect("serialize calibration report"),
    )
    .expect("write calibration report");
    Some(path)
}

#[test]
fn calibration_fixture_reports_metrics_and_checks_invariants() {
    let fixture = read_fixture();
    let development_fixture = [
        "serein-calibration-dev-relevance-noise-v1",
        "serein-calibration-dev-multilingual-v1",
    ]
    .contains(&fixture.fixture_id.as_str());
    let holdout_fixture = std::env::var("SEREIN_CALIBRATION_HOLDOUT").as_deref() == Ok("1")
        && matches!(
            fixture_path().file_name().and_then(|name| name.to_str()),
            Some("calibration_holdout_relevance_noise.json")
                | Some("calibration_holdout_v2_relevance.json")
        )
        && fixture.fixture_id.contains("holdout");
    assert!(
        development_fixture || holdout_fixture,
        "only the two development fixtures or explicitly opted-in held-out fixture are supported"
    );
    for query in &fixture.queries {
        assert!(
            query.text.chars().count() <= 512,
            "{} exceeds the recall query text limit",
            query.id
        );
        assert!(
            query.facets.len() <= 3,
            "{} exceeds the recall limit of three facets",
            query.id
        );
        assert!(
            query
                .facets
                .iter()
                .all(|facet| facet.chars().count() <= 128),
            "{} contains a facet longer than 128 characters",
            query.id
        );
    }

    let model_path = model_pack_path();
    let pack_available = model_pack_present(&model_path);
    let temp = tempdir().expect("create isolated calibration directory");
    let mut vault = Vault::open(&temp.path().join("calibration.sqlite"))
        .expect("open isolated synthetic vault");
    let source = id();
    vault
        .set_policy(
            &source,
            &Policy {
                consent: true,
                paused: false,
                recall_enabled: true,
                selected_only: false,
                selected_sites: vec![],
                excluded_sites: vec![],
                capture_epoch: 1,
            },
        )
        .expect("enable capture for synthetic fixture source");

    let timestamps = normalized_timestamps(&fixture.evidence);
    let mut event_evidence = HashMap::<String, String>::new();
    let mut events = Vec::<Event>::new();
    let mut fixture_invalid_host_ids = Vec::<String>::new();
    for item in &fixture.evidence {
        // The fixture includes a "New Tab" row without a hostname. Such a row
        // cannot cross the native Event contract and is treated as a safe drop.
        if !policy::valid_site(&item.hostname) {
            fixture_invalid_host_ids.push(item.id.clone());
            continue;
        }
        let event = make_event(
            item,
            timestamps
                .get(&item.id)
                .expect("normalized timestamp exists for fixture evidence"),
        );
        event_evidence.insert(event.event_id.clone(), item.id.clone());
        events.push(event);
    }

    let mut ingest_rejections = BTreeMap::<String, usize>::new();
    let mut accepted_evidence_ids = BTreeSet::<String>::new();
    let mut invariant_failures = Vec::<String>::new();
    for batch in events.chunks(32) {
        let response = vault
            .ingest(&source, 1, batch.to_vec())
            .expect("ingest synthetic evidence batch");
        let acknowledged: Vec<String> = response["acknowledged_ids"]
            .as_array()
            .expect("ingest response has acknowledgements")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let rejected = response["rejected"]
            .as_array()
            .expect("ingest response has rejected records");
        let mut accounted = HashSet::<String>::new();
        for event_id in &acknowledged {
            if !accounted.insert(event_id.clone()) {
                invariant_failures.push(format!("duplicate ingest acknowledgement for {event_id}"));
            }
            match event_evidence.get(event_id) {
                Some(evidence_id) => {
                    accepted_evidence_ids.insert(evidence_id.clone());
                }
                None => {
                    invariant_failures.push(format!("ingest acknowledged unknown event {event_id}"))
                }
            }
        }
        for rejection in rejected {
            let Some(event_id) = rejection["id"].as_str() else {
                invariant_failures.push("ingest rejection had no event ID".to_owned());
                continue;
            };
            if !accounted.insert(event_id.to_owned()) {
                invariant_failures.push(format!(
                    "event {event_id} was both acknowledged and rejected"
                ));
            }
            if !event_evidence.contains_key(event_id) {
                invariant_failures.push(format!("ingest rejected unknown event {event_id}"));
            }
            let reason = rejection["reason"].as_str().unwrap_or("UNKNOWN");
            *ingest_rejections.entry(reason.to_owned()).or_default() += 1;
        }
        if accounted.len() != batch.len() {
            invariant_failures.push(format!(
                "ingest accounted for {} of {} events in a batch",
                accounted.len(),
                batch.len()
            ));
        }
    }

    let accepted_by_id: HashMap<&str, &FixtureEvidence> = fixture
        .evidence
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let mut atom_evidence = HashMap::<String, BTreeSet<String>>::new();
    let mut evidence_atom = HashMap::<String, String>::new();
    for evidence_id in &accepted_evidence_ids {
        let item = accepted_by_id
            .get(evidence_id.as_str())
            .expect("accepted evidence ID came from fixture");
        match atom_for_evidence(&vault, &source, item) {
            Ok(atom) => {
                atom_evidence
                    .entry(atom.clone())
                    .or_default()
                    .insert(evidence_id.clone());
                evidence_atom.insert(evidence_id.clone(), atom);
            }
            Err(error) => invariant_failures.push(format!(
                "accepted evidence {evidence_id} has no atom mapping: {error}"
            )),
        }
    }

    let database_atom_count: i64 = vault
        .conn
        .query_row(
            "SELECT count(*) FROM atoms WHERE source=?1",
            [&source],
            |row| row.get(0),
        )
        .expect("count synthetic atoms");
    if database_atom_count as usize != atom_evidence.len() {
        invariant_failures.push(format!(
            "isolated vault has {database_atom_count} atoms but fixture mapped {}",
            atom_evidence.len()
        ));
    }

    let mut query_reports = Vec::<Value>::new();
    let mut latencies_ms = Vec::<f64>::new();
    let mut packet_sizes = Vec::<usize>::new();
    let mut precision_values = Vec::<f64>::new();
    let mut recall_values = Vec::<f64>::new();
    let mut mrr_values = Vec::<f64>::new();
    let mut ndcg_values = Vec::<f64>::new();
    let mut empty_queries = 0usize;
    let mut empty_true_negatives = 0usize;
    let mut empty_false_positives = Vec::<Value>::new();
    let mut hybrid_queries = 0usize;
    let mut lexical_queries = 0usize;
    let mut invariant_check_counts = BTreeMap::<String, usize>::new();

    for query in &fixture.queries {
        let request = Recall {
            protocol: 1,
            request_id: id(),
            client: "calibration-evaluation".to_owned(),
            vault: "default".to_owned(),
            query: query.text.clone(),
            facets: query.facets.clone(),
            scope: vec!["research".to_owned(), "projects".to_owned()],
            max_bytes: PACKET_LIMIT,
            budget_ms: RECALL_BUDGET_MS,
        };
        let started = Instant::now();
        let response = vault
            .recall(&request, &source, &model_path)
            .unwrap_or_else(|error| panic!("{} recall failed: {error}", query.id));
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        latencies_ms.push(elapsed_ms);

        let packet_bytes = serde_json::to_vec(&response)
            .expect("recall response serializes")
            .len();
        packet_sizes.push(packet_bytes);
        if packet_bytes <= request.max_bytes {
            *invariant_check_counts
                .entry("packet_within_requested_byte_limit".into())
                .or_default() += 1;
        } else {
            invariant_failures.push(format!(
                "{} packet is {packet_bytes} bytes, above {} byte limit",
                query.id, request.max_bytes
            ));
        }
        if response["request_id"] == request.request_id {
            *invariant_check_counts
                .entry("request_id_preserved".into())
                .or_default() += 1;
        } else {
            invariant_failures.push(format!("{} response request ID did not match", query.id));
        }

        let mode = response["index"]["mode"].as_str().unwrap_or("unknown");
        match mode {
            "hybrid" => hybrid_queries += 1,
            "lexical" => lexical_queries += 1,
            _ => {
                invariant_failures.push(format!("{} reported unknown index mode {mode}", query.id))
            }
        }
        let context = response["context"]
            .as_array()
            .unwrap_or_else(|| panic!("{} response has no context array", query.id));
        if context.len() <= CONTEXT_K {
            *invariant_check_counts
                .entry("context_at_most_six_records".into())
                .or_default() += 1;
        } else {
            invariant_failures.push(format!(
                "{} returned {} context records",
                query.id,
                context.len()
            ));
        }

        let expected: BTreeSet<String> = query.expected_ids.iter().cloned().collect();
        let unavailable_expected: BTreeSet<String> = expected
            .iter()
            .filter(|evidence_id| !evidence_atom.contains_key(evidence_id.as_str()))
            .cloned()
            .collect();
        let expected_atom_ids: BTreeSet<String> = expected
            .iter()
            .filter_map(|evidence_id| evidence_atom.get(evidence_id).cloned())
            .collect();
        let mut seen_atoms = HashSet::<String>::new();
        let mut returned_atom_ids = Vec::<String>::new();
        let mut returned_evidence_ids = BTreeSet::<String>::new();
        let mut matched_evidence_ids = BTreeSet::<String>::new();
        let mut matched_atom_ids = BTreeSet::<String>::new();
        let mut irrelevant_atom_ids = Vec::<String>::new();
        let mut unknown_atom_ids = Vec::<String>::new();
        let mut ranked_results = Vec::<Value>::new();
        let mut relevances = Vec::<bool>::new();
        let mut first_relevant_rank = None;

        for (rank, record) in context.iter().take(CONTEXT_K).enumerate() {
            let atom_id = record["id"].as_str().unwrap_or_default().to_owned();
            if atom_id.is_empty() {
                invariant_failures
                    .push(format!("{} returned a record without an atom ID", query.id));
                continue;
            }
            if !seen_atoms.insert(atom_id.clone()) {
                invariant_failures
                    .push(format!("{} returned duplicate atom ID {atom_id}", query.id));
            }
            returned_atom_ids.push(atom_id.clone());
            match atom_evidence.get(&atom_id) {
                Some(supported) => {
                    returned_evidence_ids.extend(supported.iter().cloned());
                    let matched: BTreeSet<String> =
                        supported.intersection(&expected).cloned().collect();
                    let relevant = !matched.is_empty();
                    if relevant && first_relevant_rank.is_none() {
                        first_relevant_rank = Some(rank + 1);
                    }
                    if relevant {
                        matched_atom_ids.insert(atom_id.clone());
                    }
                    matched_evidence_ids.extend(matched.iter().cloned());
                    relevances.push(relevant);
                    if !relevant {
                        irrelevant_atom_ids.push(atom_id.clone());
                    }
                    ranked_results.push(json!({
                        "rank":rank + 1,
                        "atom_id":atom_id,
                        "evidence_ids":supported,
                        "matched_expected_evidence_ids":matched,
                        "relevant":relevant,
                    }));
                }
                None => {
                    invariant_failures
                        .push(format!("{} returned unmapped atom ID {atom_id}", query.id));
                    unknown_atom_ids.push(atom_id);
                    relevances.push(false);
                    ranked_results.push(json!({
                        "rank":rank + 1,
                        "atom_id":returned_atom_ids.last(),
                        "evidence_ids":[],
                        "matched_expected_evidence_ids":[],
                        "relevant":false,
                    }));
                }
            }
            if record["state"] == "observed" && record["subject"] == "unknown" {
                *invariant_check_counts
                    .entry("observed_evidence_has_unknown_subject".into())
                    .or_default() += 1;
            } else {
                invariant_failures.push(format!(
                    "{} returned evidence with unexpected state/subject {:?}/{:?}",
                    query.id, record["state"], record["subject"]
                ));
            }
        }

        // Fixture event IDs can collapse into one canonical atom (for example,
        // repeated visits to the same title/site). Retrieval metrics therefore
        // use unique atoms as their expected units; expected labels without an
        // ingestible atom remain explicit unmatched units.
        let expected_atom_count = expected_atom_ids.len() + unavailable_expected.len();
        let relevant_atom_count = expected_atom_count;
        let query_recall = if expected.is_empty() {
            None
        } else {
            Some(matched_atom_ids.len() as f64 / expected_atom_count as f64)
        };
        let hit_count = relevances.iter().filter(|relevant| **relevant).count();
        let query_precision = if expected.is_empty() {
            None
        } else {
            Some(hit_count as f64 / CONTEXT_K as f64)
        };
        let query_mrr = if expected.is_empty() {
            None
        } else {
            Some(first_relevant_rank.map_or(0.0, |rank| 1.0 / rank as f64))
        };
        let ideal_count = relevant_atom_count.min(CONTEXT_K);
        let ideal_dcg = (0..ideal_count)
            .map(|rank| 1.0 / ((rank + 2) as f64).log2())
            .sum::<f64>();
        let query_ndcg = if expected.is_empty() {
            None
        } else if ideal_dcg == 0.0 {
            Some(0.0)
        } else {
            Some(dcg(relevances.iter().copied()) / ideal_dcg)
        };

        if query.expected_empty != expected.is_empty() {
            invariant_failures.push(format!(
                "{} expected_empty flag disagrees with expected ID list",
                query.id
            ));
        }
        if query.expected_empty {
            empty_queries += 1;
            if context.is_empty() {
                empty_true_negatives += 1;
            } else {
                empty_false_positives.push(json!({
                    "query_id":query.id,
                    "returned_atom_ids":returned_atom_ids.clone(),
                    "returned_evidence_ids":returned_evidence_ids.clone(),
                    "unknown_atom_ids":unknown_atom_ids.clone(),
                }));
            }
        } else {
            recall_values.push(query_recall.expect("non-empty query recall"));
            precision_values.push(query_precision.expect("non-empty query precision"));
            mrr_values.push(query_mrr.expect("non-empty query MRR"));
            ndcg_values.push(query_ndcg.expect("non-empty query nDCG"));
        }

        query_reports.push(json!({
            "query_id":query.id,
            "query":query.text,
            "expected_empty":query.expected_empty,
            "expected_evidence_ids":expected,
            "expected_canonical_atom_ids":expected_atom_ids,
            "unavailable_expected_evidence_ids":unavailable_expected,
            "returned_atom_ids":returned_atom_ids,
            "ranked_results":ranked_results,
            "returned_evidence_ids":returned_evidence_ids,
            "matched_evidence_ids":matched_evidence_ids,
            "matched_canonical_atom_ids":matched_atom_ids,
            "irrelevant_returned_atom_ids":irrelevant_atom_ids,
            "unknown_atom_ids":unknown_atom_ids,
            "recall_at_6":query_recall,
            "precision_at_6":query_precision,
            "mrr":query_mrr,
            "ndcg_at_6":query_ndcg,
            "response_status":response["status"],
            "index_mode":mode,
            "packet_bytes":packet_bytes,
            "latency_ms":elapsed_ms,
        }));
    }

    let specificity = if empty_queries == 0 {
        None
    } else {
        Some(empty_true_negatives as f64 / empty_queries as f64)
    };
    // These are regression floors for constructed development cases, not a
    // claim of field accuracy. Held-out cases are reported without tuning.
    let development_quality_gate = if pack_available && development_fixture {
        let recall = average(&recall_values).unwrap_or(0.0);
        let empty_specificity = specificity.unwrap_or(0.0);
        Some(json!({
            "minimum_recall_at_6":0.60,
            "minimum_empty_specificity":1.0,
            "passed":recall >= 0.60 && empty_specificity == 1.0,
        }))
    } else {
        None
    };
    let report = json!({
        "fixture_id":fixture.fixture_id,
        "synthetic_only":true,
        "evaluation_split":if holdout_fixture{"held-out"}else{"development"},
        "model_pack":if std::env::var_os("SEREIN_CALIBRATION_MODEL_PACK").is_some(){"override"}else{"bundled"},
        "model_pack_available":pack_available,
        "retrieval_mode_counts":{"hybrid":hybrid_queries,"lexical":lexical_queries},
        "development_quality_gate":development_quality_gate,
        "ingest":{
            "fixture_evidence_count":fixture.evidence.len(),
            "ingested_event_count":events.len(),
            "accepted_evidence_count":accepted_evidence_ids.len(),
            "unique_atom_count":atom_evidence.len(),
            "invalid_hostname_safe_drop_ids":fixture_invalid_host_ids,
            "policy_rejection_counts":ingest_rejections,
        },
        "aggregate":{
            "evaluated_query_count":fixture.queries.len(),
            "non_empty_query_count":recall_values.len(),
            "expected_empty_query_count":empty_queries,
            "recall_at_6_macro_non_empty":average(&recall_values),
            "precision_at_6_macro_non_empty":average(&precision_values),
            "mrr_macro_non_empty":average(&mrr_values),
            "ndcg_at_6_macro_non_empty":average(&ndcg_values),
            "expected_empty_specificity":specificity,
            "expected_empty_true_negatives":empty_true_negatives,
            "expected_empty_false_positives":empty_false_positives,
            "latency_ms":{"mean":average(&latencies_ms),"median":percentile(&latencies_ms,0.5),"p95":percentile(&latencies_ms,0.95)},
            "packet_bytes":{"mean":average(&packet_sizes.iter().map(|n| *n as f64).collect::<Vec<_>>()),"max":packet_sizes.iter().copied().max()},
        },
        "invariant_checks":{
            "packet_limit_bytes":PACKET_LIMIT,
            "context_limit_records":CONTEXT_K,
            "checks_passed":invariant_check_counts,
            "failures":invariant_failures,
        },
        "metric_definitions":{
            "recall_at_6":"Unique expected canonical atoms represented by the top six returned atoms divided by unique expected canonical atoms. Expected labels without an ingestible atom are counted as unmatched units.",
            "precision_at_6":"Relevant returned atom positions divided by six, even when fewer than six atoms are returned.",
            "mrr":"Reciprocal rank of the first returned atom that supports any expected evidence ID.",
            "ndcg_at_6":"Binary relevance nDCG over returned atoms; expected duplicate observations that share one atom count as one retrievable item.",
            "aggregation":"Macro average across non-empty queries. Expected-empty queries are measured separately with specificity.",
        },
        "queries":query_reports,
    });

    let report_path = write_report(&report);
    let aggregate = &report["aggregate"];
    println!(
        "CALIBRATION_REPORT path={} mode={} Recall@6={:.4} Precision@6={:.4} MRR={:.4} nDCG@6={:.4} empty_specificity={:.4} latency_p95_ms={:.2} packet_max_bytes={}",
        report_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "stdout".to_owned()),
        if pack_available { "hybrid-pack" } else { "lexical-fallback" },
        aggregate["recall_at_6_macro_non_empty"].as_f64().unwrap_or(0.0),
        aggregate["precision_at_6_macro_non_empty"].as_f64().unwrap_or(0.0),
        aggregate["mrr_macro_non_empty"].as_f64().unwrap_or(0.0),
        aggregate["ndcg_at_6_macro_non_empty"].as_f64().unwrap_or(0.0),
        aggregate["expected_empty_specificity"].as_f64().unwrap_or(0.0),
        aggregate["latency_ms"]["p95"].as_f64().unwrap_or(0.0),
        aggregate["packet_bytes"]["max"].as_u64().unwrap_or(0),
    );
    for query in report["queries"].as_array().expect("query reports") {
        let evidence_ids = query["returned_evidence_ids"]
            .as_array()
            .expect("returned IDs")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let relevant_ids = query["matched_evidence_ids"]
            .as_array()
            .expect("matched evidence IDs")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let mut irrelevant = BTreeSet::<String>::new();
        if let Some(results) = query["ranked_results"].as_array() {
            for result in results.iter().filter(|result| result["relevant"] == false) {
                if let Some(ids) = result["evidence_ids"].as_array() {
                    irrelevant.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
                }
            }
        }
        let irrelevant_ids = irrelevant.into_iter().collect::<Vec<_>>().join(",");
        println!(
            "{} returned_evidence=[{}] relevant=[{}] irrelevant=[{}] recall@6={} precision@6={} mrr={} ndcg@6={}",
            query["query_id"].as_str().unwrap_or("?"),
            evidence_ids,
            relevant_ids,
            irrelevant_ids,
            query["recall_at_6"].as_f64().map(|v| format!("{v:.3}")).unwrap_or_else(|| "n/a".into()),
            query["precision_at_6"].as_f64().map(|v| format!("{v:.3}")).unwrap_or_else(|| "n/a".into()),
            query["mrr"].as_f64().map(|v| format!("{v:.3}")).unwrap_or_else(|| "n/a".into()),
            query["ndcg_at_6"].as_f64().map(|v| format!("{v:.3}")).unwrap_or_else(|| "n/a".into()),
        );
    }
    assert!(
        report["invariant_checks"]["failures"]
            .as_array()
            .expect("invariant failure list")
            .is_empty(),
        "calibration packet, provenance, or storage invariant failed; report: {}",
        report_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "see captured test output".to_owned())
    );
    if pack_available && development_fixture {
        assert_eq!(
            report["development_quality_gate"]["passed"],
            true,
            "constructed development recall or abstention regression; report: {}",
            report_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "see captured test output".to_owned())
        );
    }
}
