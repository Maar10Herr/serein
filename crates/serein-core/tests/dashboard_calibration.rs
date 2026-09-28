//! Independent, constructed development labels for observation prominence.
//! This scores the display classifier; it does not claim real-user accuracy.

use chrono::DateTime;
use serde::Deserialize;
use serde_json::json;
use serein_core::importance;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
struct EvidenceFixture {
    evidence: Vec<Evidence>,
}

#[derive(Deserialize)]
struct Evidence {
    id: String,
    title: String,
    hostname: String,
    kind: String,
    dwell_seconds: i64,
    observed_at: String,
}

#[derive(Deserialize)]
struct LabelFixture {
    fixture_id: String,
    labels: Vec<Label>,
}

#[derive(Deserialize)]
struct Label {
    fixture: String,
    evidence_id: String,
    prominent: bool,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn prominence_reports_constructed_development_labels() {
    let root = root();
    let fixture_dir = root.join("tests/fixtures");
    let labels: LabelFixture = serde_json::from_slice(
        &fs::read(fixture_dir.join("calibration_dev_dashboard_labels.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        labels.fixture_id,
        "serein-calibration-dev-dashboard-labels-v1"
    );
    let mut confusion = [0usize; 4]; // TP, FP, TN, FN.
    let mut evaluated = 0usize;
    let mut misses = Vec::new();
    for name in [
        "calibration_dev_relevance_noise.json",
        "calibration_dev_multilingual.json",
    ] {
        let fixture: EvidenceFixture =
            serde_json::from_slice(&fs::read(fixture_dir.join(name)).unwrap()).unwrap();
        let by_id: HashMap<_, _> = fixture
            .evidence
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect();
        let mut sessions = HashMap::<(&str, &str, &str), HashSet<i64>>::new();
        let mut seconds = HashMap::<(&str, &str, &str), i64>::new();
        for item in &fixture.evidence {
            let key = (
                item.hostname.as_str(),
                item.title.as_str(),
                item.kind.as_str(),
            );
            let timestamp = DateTime::parse_from_rfc3339(&item.observed_at)
                .expect("fixture timestamp")
                .timestamp();
            sessions.entry(key).or_default().insert(timestamp / 1800);
            *seconds.entry(key).or_default() += item.dwell_seconds;
        }
        for label in labels.labels.iter().filter(|label| label.fixture == name) {
            let item = by_id
                .get(label.evidence_id.as_str())
                .expect("label evidence exists");
            let key = (
                item.hostname.as_str(),
                item.title.as_str(),
                item.kind.as_str(),
            );
            let is_search = item.kind == "search";
            let predicted = importance::prominent(
                &item.title,
                is_search.then_some(item.title.as_str()),
                seconds[&key].min(3600),
                sessions[&key].len() as i64,
                false,
            );
            match (predicted, label.prominent) {
                (true, true) => confusion[0] += 1,
                (true, false) => confusion[1] += 1,
                (false, false) => confusion[2] += 1,
                (false, true) => confusion[3] += 1,
            }
            if predicted != label.prominent {
                misses.push(json!({"fixture":name,"evidence_id":item.id,"predicted":predicted,"label":label.prominent}));
            }
            evaluated += 1;
        }
    }
    assert_eq!(
        evaluated,
        labels.labels.len(),
        "every dev label was evaluated"
    );
    let [tp, fp, tn, fn_] = confusion;
    let report = json!({
        "fixture_id":labels.fixture_id,
        "synthetic_only":true,
        "unit":"labeled event with canonical atom's accumulated dwell and independent half-hour session buckets",
        "evaluated":evaluated,
        "true_positive":tp,
        "false_positive":fp,
        "true_negative":tn,
        "false_negative":fn_,
        "precision":tp as f64/(tp+fp).max(1) as f64,
        "recall":tp as f64/(tp+fn_).max(1) as f64,
        "specificity":tn as f64/(tn+fp).max(1) as f64,
        "disagreements":misses,
    });
    if let Some(path) = std::env::var_os("SEREIN_DASHBOARD_CALIBRATION_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("DASHBOARD_CALIBRATION {}", report);
    assert!(
        tp > 0 && tn > 0,
        "development fixture must exercise both classes"
    );
}

#[test]
fn prominence_reports_held_out_constructed_labels_when_requested() {
    if std::env::var("SEREIN_DASHBOARD_HOLDOUT").as_deref() != Ok("1") {
        return;
    }
    let fixture_dir = root().join("tests/fixtures");
    let labels: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture_dir.join("calibration_holdout_dashboard_noise.json")).unwrap(),
    )
    .unwrap();
    let relevance: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture_dir.join("calibration_holdout_relevance_noise.json")).unwrap(),
    )
    .unwrap();
    let observations = labels["observations"].as_array().unwrap();
    let metadata: HashMap<_, _> = relevance["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["id"].as_str().unwrap(), &item["evidence"]))
        .collect();
    let mut sessions = HashMap::<(String, String, String), HashSet<String>>::new();
    let mut seconds = HashMap::<(String, String, String), i64>::new();
    for item in observations {
        let id = item["observation_id"].as_str().unwrap();
        let evidence = metadata[id];
        let key = (
            item["host"].as_str().unwrap().to_owned(),
            item["title"].as_str().unwrap().to_owned(),
            evidence["query"].as_str().unwrap_or("").to_owned(),
        );
        sessions
            .entry(key.clone())
            .or_default()
            .insert(item["session_id"].as_str().unwrap().to_owned());
        *seconds.entry(key).or_default() += item["dwell_seconds"].as_i64().unwrap();
    }
    let mut confusion = [0usize; 4]; // TP, FP, TN, FN.
    let mut disagreements = Vec::new();
    for item in observations {
        let id = item["observation_id"].as_str().unwrap();
        let evidence = metadata[id];
        let query = evidence["query"].as_str().filter(|value| !value.is_empty());
        assert_eq!(item["query_present"].as_bool().unwrap(), query.is_some());
        let key = (
            item["host"].as_str().unwrap().to_owned(),
            item["title"].as_str().unwrap().to_owned(),
            query.unwrap_or("").to_owned(),
        );
        let predicted = importance::prominent(
            &key.1,
            query,
            seconds[&key].min(3600),
            sessions[&key].len() as i64,
            false,
        );
        let expected = item["expected_dashboard_treatment"] == "prominent";
        match (predicted, expected) {
            (true, true) => confusion[0] += 1,
            (true, false) => confusion[1] += 1,
            (false, false) => confusion[2] += 1,
            (false, true) => confusion[3] += 1,
        }
        if predicted != expected {
            disagreements.push(
                json!({"observation_id":id,"predicted":predicted,"expected_prominent":expected}),
            );
        }
    }
    let [tp, fp, tn, fn_] = confusion;
    let report = json!({
        "fixture_id":labels["fixture_id"],
        "evaluation_split":"held-out",
        "synthetic_only":true,
        "evaluated":observations.len(),
        "true_positive":tp,
        "false_positive":fp,
        "true_negative":tn,
        "false_negative":fn_,
        "precision":tp as f64/(tp+fp).max(1) as f64,
        "recall":tp as f64/(tp+fn_).max(1) as f64,
        "specificity":tn as f64/(tn+fp).max(1) as f64,
        "disagreements":disagreements,
    });
    if let Some(path) = std::env::var_os("SEREIN_DASHBOARD_HOLDOUT_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("DASHBOARD_HOLDOUT {}", report);
}
