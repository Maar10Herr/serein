//! Descriptive retrieval evaluation over constructed public-page research journeys.
//! The labels are illustrative, not a measured field-quality claim.
use chrono::{Duration, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

use serein_core::{id, storage::Vault, Event, Policy, Recall};

#[derive(Deserialize)]
struct Journey {
    schema_version: u32,
    topic: String,
    seed_fingerprint: String,
    evidence: Vec<Evidence>,
    queries: Vec<Query>,
}

#[derive(Deserialize)]
struct Evidence {
    id: String,
    title: String,
    site_key: String,
    search_query: Option<String>,
    days_ago: i64,
    sessions: Sessions,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Sessions {
    Count(usize),
    Labels(Vec<String>),
}

impl Sessions {
    fn count(&self) -> usize {
        match self {
            Self::Count(count) => *count,
            Self::Labels(labels) => labels.len(),
        }
    }
}

#[derive(Deserialize)]
struct Query {
    id: String,
    query: String,
    facets: Vec<String>,
    expected_ids: Vec<String>,
    kind: String,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn journey_paths() -> Vec<PathBuf> {
    let dir = root().join("tests/fixtures");
    let mut paths: Vec<_> = fs::read_dir(dir)
        .expect("journey fixture directory")
        .map(|entry| entry.expect("fixture directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("eval_journey_") && name.ends_with(".json"))
        })
        .collect();
    if let Ok(filter) = std::env::var("SEREIN_JOURNEY_FILTER") {
        let allowed: HashSet<_> = filter.split(',').map(str::trim).collect();
        paths.retain(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.strip_prefix("eval_journey_"))
                .is_some_and(|suffix| allowed.contains(suffix))
        });
    }
    paths.sort();
    assert!(!paths.is_empty(), "at least one journey is required");
    paths
}

fn run_journey(path: PathBuf) -> Value {
    let raw = fs::read_to_string(&path).expect("read journey fixture");
    let journey: Journey = serde_json::from_str(&raw).expect("parse journey fixture");
    assert_eq!(journey.schema_version, 1);
    assert_eq!(journey.seed_fingerprint.len(), 16);
    assert!(!journey.topic.is_empty());
    assert!(journey.evidence.len() >= 12);
    assert!(journey.queries.len() >= 8);
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let temp = tempdir().expect("isolated journey vault");
    let mut vault = Vault::open(&temp.path().join("journey.sqlite")).expect("open journey vault");
    let source = id();
    vault
        .set_policy(
            &source,
            &Policy {
                consent: true,
                recall_enabled: true,
                capture_epoch: 1,
                ..Policy::default()
            },
        )
        .expect("enable test consent");
    let mut atom_for_id = HashMap::<String, String>::new();
    let mut seen_ids = HashSet::new();
    for evidence in &journey.evidence {
        assert!(
            seen_ids.insert(evidence.id.clone()),
            "duplicate evidence ID"
        );
        let sessions = evidence.sessions.count();
        assert!((1..=6).contains(&sessions));
        assert!((0..=80).contains(&evidence.days_ago));
        assert!(evidence.days_ago + sessions as i64 <= 89);
        let events: Vec<_> = (0..sessions)
            .map(|index| Event {
                event_id: id(),
                visit_id: id(),
                site_key: evidence.site_key.clone(),
                site_epoch: 0,
                observed_at: (Utc::now() - Duration::days(evidence.days_ago + index as i64))
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
                kind: if evidence.search_query.is_some() {
                    "search"
                } else {
                    "visit"
                }
                .into(),
                title: evidence.title.clone(),
                search_query: evidence.search_query.clone(),
                foreground_seconds: if evidence.search_query.is_some() {
                    2
                } else {
                    15
                },
            })
            .collect();
        let result = vault
            .ingest(&source, 1, events)
            .expect("ingest journey events");
        assert_eq!(
            result["acknowledged_ids"].as_array().map(Vec::len),
            Some(sessions),
            "all constructed evidence should be eligible: {}",
            evidence.id
        );
        let atom: String = vault
            .conn
            .query_row(
                "SELECT id FROM atoms WHERE source=?1 ORDER BY rowid DESC LIMIT 1",
                rusqlite::params![source],
                |row| row.get(0),
            )
            .expect("map observation to atom");
        atom_for_id.insert(evidence.id.clone(), atom);
    }
    let reverse: HashMap<_, _> = atom_for_id
        .iter()
        .map(|(evidence_id, atom_id)| (atom_id.clone(), evidence_id.clone()))
        .collect();
    assert_eq!(
        reverse.len(),
        journey.evidence.len(),
        "distinct fixture evidence must not collapse to one canonical atom"
    );
    let model_path = root().join("models/pack");
    let mut results = Vec::new();
    let mut positive = 0usize;
    let mut recall_sum = 0.0;
    let mut precision_sum = 0.0;
    let mut mrr_sum = 0.0;
    let mut ndcg_sum = 0.0;
    let mut empty_count = 0usize;
    let mut correct_empty = 0usize;
    let mut unexpected_attribution = 0usize;
    let mut duplicate_ids = 0usize;
    let mut byte_failures = 0usize;
    for query in &journey.queries {
        assert!(query.facets.len() <= 3, "too many facets: {}", query.id);
        let expected: HashSet<_> = query.expected_ids.iter().cloned().collect();
        assert_eq!(expected.len(), query.expected_ids.len());
        for evidence_id in &expected {
            assert!(atom_for_id.contains_key(evidence_id), "unknown expected ID");
        }
        let request = Recall {
            protocol: 1,
            request_id: id(),
            client: "journey-evaluation".into(),
            vault: "default".into(),
            query: query.query.clone(),
            facets: query.facets.clone(),
            scope: vec!["research".into()],
            max_bytes: 4096,
            budget_ms: 1500,
        };
        let response = vault
            .recall(&request, &source, &model_path)
            .expect("journey recall");
        let bytes = serde_json::to_vec(&response).unwrap().len();
        if bytes > request.max_bytes {
            byte_failures += 1;
        }
        let records = response["context"].as_array().expect("context array");
        let mut returned = Vec::new();
        let mut seen_atoms = HashSet::new();
        for record in records {
            let atom_id = record["id"].as_str().expect("returned atom ID");
            if !seen_atoms.insert(atom_id) {
                duplicate_ids += 1;
            }
            if record["state"] != "observed" || record["subject"] != "unknown" {
                unexpected_attribution += 1;
            }
            returned.push(reverse.get(atom_id).expect("fixture atom mapping").clone());
        }
        let relevant = returned.iter().filter(|id| expected.contains(*id)).count();
        if expected.is_empty() {
            empty_count += 1;
            if returned.is_empty() {
                correct_empty += 1;
            }
        } else {
            positive += 1;
            recall_sum += relevant as f64 / expected.len() as f64;
            precision_sum += relevant as f64 / returned.len().max(1) as f64;
            mrr_sum += returned
                .iter()
                .position(|id| expected.contains(id))
                .map_or(0.0, |rank| 1.0 / (rank + 1) as f64);
            let dcg: f64 = returned
                .iter()
                .enumerate()
                .filter(|(_, id)| expected.contains(*id))
                .map(|(rank, _)| 1.0 / ((rank + 2) as f64).log2())
                .sum();
            let ideal: f64 = (0..expected.len().min(6))
                .map(|rank| 1.0 / ((rank + 2) as f64).log2())
                .sum();
            ndcg_sum += dcg / ideal;
        }
        results.push(json!({
            "id": query.id,
            "kind": query.kind,
            "expected_ids": query.expected_ids,
            "returned_ids": returned,
            "status": response["status"],
            "index_mode": response["index"]["mode"],
            "packet_bytes": bytes,
            "relevant_count": relevant,
        }));
    }
    assert_eq!(unexpected_attribution, 0);
    assert_eq!(duplicate_ids, 0);
    assert_eq!(byte_failures, 0);
    let denominator = positive.max(1) as f64;
    json!({
        "fixture": name,
        "topic": journey.topic,
        "evidence_count": journey.evidence.len(),
        "query_count": journey.queries.len(),
        "positive_query_count": positive,
        "empty_query_count": empty_count,
        "metrics": {
            "mean_recall_at_6": recall_sum / denominator,
            "mean_precision_returned": precision_sum / denominator,
            "mrr": mrr_sum / denominator,
            "ndcg_at_6": ndcg_sum / denominator,
            "empty_query_specificity": correct_empty as f64 / empty_count.max(1) as f64,
        },
        "hard_invariants": {
            "unexpected_attribution_count": unexpected_attribution,
            "duplicate_atom_id_count": duplicate_ids,
            "byte_bound_failure_count": byte_failures,
        },
        "queries": results,
    })
}

#[test]
fn constructed_public_page_journeys_report_retrieval_quality() {
    let reports: Vec<_> = journey_paths().into_iter().map(run_journey).collect();
    let report = json!({
        "description": "Constructed journeys from public-page metadata with simulated recency and sessions. Hand-curated labels are illustrative, not field accuracy or user validation.",
        "model_pack": "models/pack",
        "journeys": reports,
    });
    if let Ok(path) = std::env::var("SEREIN_JOURNEY_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).expect("write journey report");
    }
}
