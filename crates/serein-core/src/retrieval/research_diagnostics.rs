//! Ignored, bounded stage-loss trace for the frozen metadata-only handoff.
//! The only imported document fields are title and hostname, through Vault's
//! public ingest API; every run creates a fresh temporary vault.

use super::*;
use rusqlite::{params, params_from_iter, Connection};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use tempfile::tempdir;

const MAX_DOCS: usize = 64;
const MAX_QUERIES: usize = 64;
const MAX_REFRESH_ROUNDS: usize = 8;
const SCAN_LIMIT: usize = 10_000;
const BUDGET_MS: u64 = 2_000;

#[derive(Deserialize)]
struct Manifest {
    dataset_name: String,
    usable_documents: usize,
    queries: usize,
    qrels: usize,
    artifacts: BTreeMap<String, Digest>,
}
#[derive(Deserialize)]
struct Digest {
    bytes: usize,
    sha256: String,
}
// These structs intentionally project only fields needed by the diagnostic.
#[derive(Deserialize)]
struct Doc {
    doc_id: String,
    title: String,
    site_key: String,
}
#[derive(Deserialize)]
struct Query {
    query_id: String,
    text: String,
}
#[derive(Deserialize)]
struct Qrel {
    query_id: String,
    doc_id: String,
    relevance: u8,
}
struct Dataset {
    name: String,
    docs: Vec<Doc>,
    queries: Vec<Query>,
    qrels: Vec<Qrel>,
    hashes: BTreeMap<String, String>,
}

fn read_limited(path: &Path, limit: u64) -> std::result::Result<Vec<u8>, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > limit {
        return Err(format!("{} exceeds its input size limit", path.display()));
    }
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn jsonl<T: DeserializeOwned>(
    path: &Path,
    limit: u64,
) -> std::result::Result<(Vec<T>, Vec<u8>), String> {
    let bytes = read_limited(path, limit)?;
    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for (line, text) in text
        .lines()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
    {
        rows.push(
            serde_json::from_str(text)
                .map_err(|e| format!("{} line {}: {e}", path.display(), line + 1))?,
        );
    }
    Ok((rows, bytes))
}

fn load_dataset(dir: &Path) -> std::result::Result<Dataset, String> {
    let manifest: Manifest =
        serde_json::from_slice(&read_limited(&dir.join("manifest.json"), 128 * 1024)?)
            .map_err(|e| e.to_string())?;
    let (docs, corpus): (Vec<Doc>, Vec<u8>) = jsonl(&dir.join("corpus.jsonl"), 512 * 1024)?;
    let (queries, query_bytes): (Vec<Query>, Vec<u8>) =
        jsonl(&dir.join("queries.jsonl"), 256 * 1024)?;
    let (qrels, qrel_bytes): (Vec<Qrel>, Vec<u8>) =
        jsonl(&dir.join("qrels.jsonl"), 2 * 1024 * 1024)?;
    if docs.is_empty()
        || docs.len() > MAX_DOCS
        || queries.is_empty()
        || queries.len() > MAX_QUERIES
        || qrels.len() > MAX_DOCS * MAX_QUERIES
        || (manifest.usable_documents, manifest.queries, manifest.qrels)
            != (docs.len(), queries.len(), qrels.len())
    {
        return Err(
            "Dataset counts exceed the diagnostic bounds or disagree with the manifest".into(),
        );
    }
    let files = [
        ("corpus.jsonl", corpus),
        ("queries.jsonl", query_bytes),
        ("qrels.jsonl", qrel_bytes),
    ];
    let mut hashes = BTreeMap::new();
    for (name, bytes) in files {
        let expected = manifest
            .artifacts
            .get(name)
            .ok_or_else(|| format!("Manifest omits {name}"))?;
        let actual = crate::hash(&bytes);
        if expected.bytes != bytes.len() || expected.sha256 != actual {
            return Err(format!("Frozen {name} size/hash does not match manifest"));
        }
        hashes.insert(name.to_owned(), actual);
    }
    let doc_ids: HashSet<_> = docs.iter().map(|d| d.doc_id.as_str()).collect();
    let query_ids: HashSet<_> = queries.iter().map(|q| q.query_id.as_str()).collect();
    let mut payloads = HashSet::new();
    if doc_ids.len() != docs.len()
        || query_ids.len() != queries.len()
        || docs.iter().any(|d| {
            d.title.chars().count() > 256
                || !crate::policy::valid_site(&d.site_key)
                || !payloads.insert(format!("{}\0{}", d.site_key, d.title))
        })
        || queries.iter().any(|q| q.text.chars().count() > 512)
    {
        return Err(
            "Dataset contains duplicate IDs/payloads or fields outside native limits".into(),
        );
    }
    let mut pairs = HashSet::new();
    if qrels.iter().any(|q| {
        q.relevance > 2
            || !doc_ids.contains(q.doc_id.as_str())
            || !query_ids.contains(q.query_id.as_str())
            || !pairs.insert((q.query_id.as_str(), q.doc_id.as_str()))
    }) || qrels.len() != docs.len() * queries.len()
    {
        return Err(
            "Qrels are invalid, duplicated, or not a complete query × document matrix".into(),
        );
    }
    Ok(Dataset {
        name: manifest.dataset_name,
        docs,
        queries,
        qrels,
        hashes,
    })
}

fn doc_ids(ids: &[String], atom_to_doc: &HashMap<String, String>) -> Vec<String> {
    ids.iter()
        .map(|id| {
            atom_to_doc
                .get(id)
                .expect("stage ID belongs to temporary fixture")
                .clone()
        })
        .collect()
}

fn rank_ids(ranks: &HashMap<String, usize>) -> Vec<String> {
    let mut rows: Vec<_> = ranks.iter().map(|(id, rank)| (*rank, id.clone())).collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    rows.into_iter().map(|(_, id)| id).collect()
}

// Raw FTS means indexed matches before observed hard/relevance admission.
// Source, retention, site policy, and feedback suppression remain enforced.
fn raw_fts(
    conn: &Connection,
    plan: &EligibilityPlan<'_>,
    request: &Recall,
    start: Instant,
) -> Result<(Vec<String>, bool)> {
    let terms = tokens(request, &plan.tokenizer)?;
    if terms.is_empty() || deadline_reached(start, BUDGET_MS) {
        return Ok((vec![], false));
    }
    let mut args = vec![
        lexical_fts_query(&terms),
        plan.source.to_owned(),
        plan.retention_cutoff.clone(),
    ];
    let filter = site_filter_sql(plan.policy, &mut args);
    let sql = format!("SELECT a.id FROM atom_fts JOIN atoms a ON a.id=atom_fts.id
WHERE atom_fts MATCH ? AND a.source=? AND a.last_seen>=? {filter}
AND NOT EXISTS(SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action IN ('do_not_use','wrong_topic','confirm_constraint'))
ORDER BY bm25(atom_fts),a.id LIMIT {SCAN_LIMIT}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?;
    let mut ids = Vec::new();
    let mut expired = false;
    for row in rows {
        if deadline_reached(start, BUDGET_MS) {
            expired = true;
            break;
        }
        ids.push(row?);
    }
    Ok((ids, expired))
}

// Raw semantic means indexed vectors scored by the canonical model before
// hard eligibility, threshold, coverage admission, or channel truncation.
fn raw_semantic(
    conn: &Connection,
    plan: &EligibilityPlan<'_>,
    request: &Recall,
    encoder: &crate::model::Encoder,
    start: Instant,
) -> Result<(Vec<(String, f32)>, bool)> {
    let Some(query) = encoder.encode(&request.query) else {
        return Ok((vec![], false));
    };
    if deadline_reached(start, BUDGET_MS) {
        return Ok((vec![], true));
    }
    let mut args = vec![
        plan.source.to_owned(),
        encoder.manifest.model_hash.clone(),
        plan.retention_cutoff.clone(),
    ];
    let filter = site_filter_sql(plan.policy, &mut args);
    let sql = format!("SELECT a.id,v.vector FROM vectors v JOIN atoms a ON a.id=v.atom
WHERE a.source=? AND v.model=? AND a.last_seen>=? {filter}
AND NOT EXISTS(SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action IN ('do_not_use','wrong_topic','confirm_constraint'))
ORDER BY a.last_seen DESC,a.id LIMIT {SCAN_LIMIT}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut scores = Vec::new();
    let mut expired = false;
    for row in rows {
        if deadline_reached(start, BUDGET_MS) {
            expired = true;
            break;
        }
        let (id, bytes) = row?;
        if bytes.len() != encoder.manifest.dimensions * 4 {
            continue;
        }
        let vector: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|part| f32::from_le_bytes(part.try_into().unwrap()))
            .collect();
        let score = crate::model::cosine(&query, &vector);
        if score.is_finite() {
            scores.push((id, score));
        }
    }
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok((scores, expired))
}

fn hard_facts(
    conn: &Connection,
    plan: &EligibilityPlan<'_>,
    atom: &str,
    semantic_score: Option<f32>,
) -> Result<Value> {
    let (site, kind, title, query, last_seen): (String, String, String, Option<String>, String) =
        conn.query_row(
            "SELECT site,kind,title,query,last_seen FROM atoms WHERE id=? AND source=?",
            params![atom, plan.source],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    let evidence = CandidateEvidence::observed(
        atom.into(),
        plan.source,
        site.clone(),
        kind,
        title,
        query,
        last_seen,
        plan.site_allowed(&site),
    );
    let content = evidence.observed_content();
    let (matched, coverage) = lexical_coverage(
        &plan.main_terms,
        &evidence_tokens(&plan.tokenizer, &content)?,
    );
    let missing: Vec<_> = plan
        .exact
        .difference(&exact_constraints(&content))
        .map(|x| json!({"kind":format!("{:?}",x.kind),"normalized":&x.normalized}))
        .collect();
    Ok(json!({
        "main_coverage":{"matched_terms":matched,"term_count":plan.main_terms.len(),"fraction":coverage},
        "exact_hard_eligible":plan.observed_hard_eligible(&evidence),
        "missing_exact_constraints":missing,
        "site_allowed":evidence.site_allowed,
        "semantic_main_score":semantic_score,
        "semantic_threshold_pass":semantic_score.is_some_and(semantic_candidate_admitted)
    }))
}

fn loss_reason(
    ingested: bool,
    raw: bool,
    hard: Option<bool>,
    channel: bool,
    candidate: bool,
    packet: bool,
) -> &'static str {
    if !ingested {
        "policy_rejected_or_not_ingested"
    } else if packet {
        "returned_in_native_packet"
    } else if candidate {
        "lost_during_packet_grouping_or_caps"
    } else if channel {
        "lost_after_channel_union_during_native_candidate_stage"
    } else if hard == Some(false) {
        "rejected_by_observed_hard_eligibility"
    } else if raw {
        "raw_hit_not_admitted_or_outside_channel_capacity"
    } else {
        "no_raw_fts_match_or_semantic_vector_candidate"
    }
}

fn output_path() -> std::result::Result<PathBuf, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let value = std::env::var_os("SEREIN_RESEARCH_DIAGNOSTICS_OUTPUT").ok_or_else(|| {
        "Set SEREIN_RESEARCH_DIAGNOSTICS_OUTPUT to a new ignored JSON path".to_owned()
    })?;
    let given = PathBuf::from(value);
    let path = if given.is_absolute() {
        given
    } else {
        root.join(given)
    };
    if path.exists() {
        return Err("Refusing to overwrite an existing diagnostics file".into());
    }
    let parent = path
        .parent()
        .ok_or("Diagnostics output needs a parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let ignored = root
        .join(".implementation")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !parent.starts_with(ignored) {
        return Err("Output must be under ignored .implementation".into());
    }
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "Output must be inside repository")?;
    if !Command::new("git")
        .args(["check-ignore", "--quiet"])
        .arg(relative)
        .current_dir(root)
        .status()
        .map_err(|e| e.to_string())?
        .success()
    {
        return Err("Diagnostics output path is not ignored by Git".into());
    }
    Ok(path)
}

#[test]
#[ignore = "requires explicit handoff dataset and new ignored output paths; creates only a temporary vault"]
fn native_handoff_stage_loss_diagnostic() {
    let dataset_dir = std::env::var_os("SEREIN_RESEARCH_DATASET")
        .map(PathBuf::from)
        .expect("Set SEREIN_RESEARCH_DATASET to the frozen data directory");
    let dataset =
        load_dataset(&dataset_dir).unwrap_or_else(|e| panic!("Dataset validation failed: {e}"));
    let output = output_path().unwrap_or_else(|e| panic!("Invalid diagnostics output: {e}"));
    let model_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/pack");
    let encoder =
        crate::model::Encoder::open(&model_path).expect("canonical checked-in model pack");

    let temp = tempdir().expect("temporary vault root");
    let mut vault =
        crate::storage::Vault::open(&temp.path().join("context.sqlite")).expect("temporary vault");
    let source = crate::id();
    let policy = crate::Policy {
        consent: true,
        recall_enabled: true,
        capture_epoch: 1,
        ..Default::default()
    };
    vault
        .set_policy(&source, &policy)
        .expect("fixture source policy");
    let synthetic_time = crate::now();
    let mut event_doc = HashMap::new();
    let events: Vec<_> = dataset
        .docs
        .iter()
        .map(|doc| {
            let event_id = crate::id();
            event_doc.insert(event_id.clone(), doc.doc_id.clone());
            crate::Event {
                event_id,
                visit_id: crate::id(),
                site_key: doc.site_key.clone(),
                site_epoch: 0,
                observed_at: synthetic_time.clone(),
                kind: "visit".into(),
                title: doc.title.clone(),
                search_query: None,
                foreground_seconds: 5,
            }
        })
        .collect();
    let mut doc_atom = HashMap::new();
    let mut atom_doc = HashMap::new();
    let mut rejected = Vec::new();
    for batch in events.chunks(16) {
        let receipt = vault
            .ingest(&source, 1, batch.to_vec())
            .expect("public native ingest");
        let ack: HashSet<String> = receipt["acknowledged_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let rejects = receipt["rejected"].as_array().unwrap();
        let rejected_ids: HashSet<String> = rejects
            .iter()
            .filter_map(|x| x["id"].as_str())
            .map(str::to_owned)
            .collect();
        assert_eq!(
            ack.union(&rejected_ids).cloned().collect::<HashSet<_>>(),
            batch.iter().map(|e| e.event_id.clone()).collect()
        );
        assert!(ack.is_disjoint(&rejected_ids));
        for row in rejects {
            rejected.push(
                json!({"doc_id":event_doc[row["id"].as_str().unwrap()],"reason":row["reason"]}),
            );
        }
        for id in ack {
            let doc = &event_doc[&id];
            let atom: String = vault
                .conn
                .query_row(
                    "SELECT atom FROM events WHERE source=? AND id=?",
                    params![source, id],
                    |r| r.get(0),
                )
                .expect("event-to-atom mapping");
            assert!(doc_atom.insert(doc.clone(), atom.clone()).is_none());
            assert!(atom_doc.insert(atom, doc.clone()).is_none());
        }
    }
    assert_eq!(doc_atom.len() + rejected.len(), dataset.docs.len());
    rejected.sort_by(|a, b| a["doc_id"].as_str().cmp(&b["doc_id"].as_str()));

    let mut refresh = Vec::new();
    let mut pending = usize::MAX;
    for _ in 0..MAX_REFRESH_ROUNDS {
        let result = vault.refresh(&model_path, 5_000).expect("bounded refresh");
        pending = result["pending_atoms"].as_u64().unwrap_or(u64::MAX) as usize;
        refresh.push(json!({"status":result["status"],"processed":result["processed"],"pending_atoms":pending}));
        if pending == 0 {
            break;
        }
    }
    assert_eq!(pending, 0, "bounded refresh did not finish");

    let mut qrels: HashMap<&str, Vec<&Qrel>> = HashMap::new();
    for row in &dataset.qrels {
        qrels.entry(&row.query_id).or_default().push(row);
    }
    let mut reports = Vec::new();
    for query in &dataset.queries {
        let request = crate::Recall {
            protocol: 1,
            request_id: crate::id(),
            client: "research-diagnostics".into(),
            vault: "default".into(),
            query: query.text.clone(),
            facets: vec![],
            scope: vec!["research".into()],
            max_bytes: 16_384,
            budget_ms: BUDGET_MS,
        };
        // Run the public path on the fully indexed fixture before borrowing
        // the connection through the diagnostic eligibility plan.
        let packet = vault
            .recall(&request, &source, &model_path)
            .expect("native recall packet");
        let packet_atoms: Vec<_> = packet["context"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect();
        let analysis_now = chrono::Utc::now();
        let plan = EligibilityPlan::new_at(
            &vault.conn,
            &source,
            &policy,
            &request,
            Some(&encoder),
            Instant::now(),
            BUDGET_MS,
            analysis_now.clone(),
        )
        .expect("eligibility plan");
        let (fts_ids, fts_deadline) =
            raw_fts(&vault.conn, &plan, &request, Instant::now()).expect("raw FTS scan");
        let (semantic_scores, semantic_deadline) =
            raw_semantic(&vault.conn, &plan, &request, &encoder, Instant::now())
                .expect("raw semantic scan");

        let channel_start = Instant::now();
        let terms = tokens(&request, &plan.tokenizer).expect("query tokens");
        let lexical = lexical_ranks(&vault.conn, &plan, &terms, channel_start, BUDGET_MS)
            .expect("lexical channel");
        let semantic = semantic_ranks(
            &vault.conn,
            &plan,
            Some(&encoder),
            &request,
            channel_start,
            BUDGET_MS,
        )
        .expect("semantic channel");
        let confirmed = confirmed_ranks(&vault.conn, &plan, channel_start, BUDGET_MS)
            .expect("confirmed channel");
        let union: HashSet<_> = lexical
            .keys()
            .chain(semantic.ranks.keys())
            .chain(confirmed.keys())
            .cloned()
            .collect();

        let candidate_batch = candidates(
            &vault.conn,
            &source,
            &policy,
            &request,
            Some(&encoder),
            Instant::now(),
            BUDGET_MS,
        )
        .expect("native candidates");
        let candidate_atoms: Vec<_> = candidate_batch
            .candidates
            .iter()
            .map(|c| c.record["id"].as_str().unwrap().to_owned())
            .collect();
        let fts_rank: HashMap<_, _> = fts_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i + 1))
            .collect();
        let sem_rank: HashMap<_, _> = semantic_scores
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (id.clone(), i + 1))
            .collect();
        let sem_score: HashMap<_, _> = semantic_scores.iter().cloned().collect();
        let candidate_rank: HashMap<_, _> = candidate_atoms
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i + 1))
            .collect();
        let packet_rank: HashMap<_, _> = packet_atoms
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i + 1))
            .collect();
        let lex_ids = rank_ids(&lexical);
        let sem_ids = rank_ids(&semantic.ranks);
        let conf_ids = rank_ids(&confirmed);
        let union_ids: Vec<_> = union.iter().cloned().collect();

        let mut expected = Vec::new();
        let mut traces = Vec::new();
        for qrel in qrels
            .get(query.query_id.as_str())
            .into_iter()
            .flatten()
            .filter(|q| q.relevance > 0)
        {
            expected.push(json!({"doc_id":qrel.doc_id,"grade":qrel.relevance}));
            let atom = doc_atom.get(&qrel.doc_id);
            let facts = atom.map(|id| {
                hard_facts(&vault.conn, &plan, id, sem_score.get(id).copied())
                    .expect("hard eligibility facts")
            });
            let hard = facts
                .as_ref()
                .and_then(|v| v["exact_hard_eligible"].as_bool());
            let fr = atom.and_then(|id| fts_rank.get(id)).copied();
            let sr = atom.and_then(|id| sem_rank.get(id)).copied();
            let lr = atom.and_then(|id| lexical.get(id)).copied();
            let cr = atom.and_then(|id| semantic.ranks.get(id)).copied();
            let vr = atom.and_then(|id| confirmed.get(id)).copied();
            let channel = lr.is_some() || cr.is_some() || vr.is_some();
            let nr = atom.and_then(|id| candidate_rank.get(id)).copied();
            let pr = atom.and_then(|id| packet_rank.get(id)).copied();
            traces.push(json!({
                "doc_id":qrel.doc_id,"grade":qrel.relevance,
                "raw_fts_rank":fr,"raw_semantic_rank":sr,
                "lexical_channel_rank":lr,"semantic_channel_rank":cr,"confirmed_channel_rank":vr,
                "candidate_rank_before_packet_caps":nr,"native_packet_rank":pr,"facts":facts,
                "loss_reason":loss_reason(atom.is_some(),fr.is_some()||sr.is_some(),hard,channel,nr.is_some(),pr.is_some())
            }));
        }
        reports.push(json!({
            "query_id":query.query_id,"expected_relevant_ids":expected,"expected_traces":traces,
            "raw_fts_pre_admission":{"document_ids_bm25_order":doc_ids(&fts_ids,&atom_doc),"deadline_reached":fts_deadline,"scan_limit":SCAN_LIMIT},
            "raw_semantic_pre_admission":{"document_ids_cosine_order":doc_ids(&semantic_scores.iter().map(|x|x.0.clone()).collect::<Vec<_>>(),&atom_doc),
                "scores":semantic_scores.iter().map(|(id,score)|json!({"doc_id":atom_doc[id],"score":score})).collect::<Vec<_>>(),
                "deadline_reached":semantic_deadline,"scan_limit":SCAN_LIMIT},
            "eligible_channel_union":{"lexical":doc_ids(&lex_ids,&atom_doc),"semantic":doc_ids(&sem_ids,&atom_doc),"confirmed":doc_ids(&conf_ids,&atom_doc),"union":doc_ids(&union_ids,&atom_doc),
                "method":"production channel helpers after hard eligibility, admission, and channel caps","deadline_reached":deadline_reached(channel_start,BUDGET_MS)},
            "native_candidates_before_packet_caps":doc_ids(&candidate_atoms,&atom_doc),
            "final_native_packet":{"document_ids":doc_ids(&packet_atoms,&atom_doc),"status":packet["status"],"index":packet["index"],"caps_applied":true,"max_bytes":request.max_bytes}
        }));
    }
    let report = json!({
        "diagnostic":"native stage-loss trace; exploratory regression evidence only",
        "dataset":{"name":dataset.name,"documents":dataset.docs.len(),"queries":dataset.queries.len(),"qrels":dataset.qrels.len(),"verified_sha256":dataset.hashes},
        "fixture":{"vault":"fresh temporary SQLite vault removed at test exit","source_id":source,"ingested_atoms":doc_atom.len(),"policy_rejected_documents":rejected,
            "imported_fields":["title","site_key/hostname"],"not_imported":["URLs/paths","page body/accessibility tree","search queries","journey/split/query labels","qrels/relevance labels","observed timestamps/behavior"],
            "synthetic_timestamp":synthetic_time,"synthetic_kind":"visit","synthetic_foreground_seconds":5,"model_pack":"checked-in models/pack","model_hash":encoder.manifest.model_hash,
            "bounded_refresh":{"runs":refresh,"max_rounds":MAX_REFRESH_ROUNDS,"pending_atoms_after":pending}},
        "methods_and_limits":{"max_documents":MAX_DOCS,"max_queries":MAX_QUERIES,"query_budget_ms":BUDGET_MS,
            "raw_fts":"Production FTS MATCH + source/retention/site/suppression filters before observed eligibility/admission; max 10,000 rows.",
            "raw_semantic":"Canonical query cosine over source/retention/site/suppression filtered vectors before observed eligibility/admission; max 10,000 rows.",
            "native_candidates":"Actual candidates() output before packet de-duplication, site cap, six slots, and max_bytes.",
            "native_packet":"Actual Vault::recall output after packet caps; source labels/qrels were scoring-only.",
            "interpretation":"Small synthetic metadata pilot for stage diagnosis, not an estimate of user accuracy or causality."},
        "queries":reports
    });
    let data = serde_json::to_vec_pretty(&report).expect("serialize diagnostic report");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .unwrap_or_else(|e| panic!("Could not create ignored output {}: {e}", output.display()));
    file.write_all(&data).expect("write diagnostic report");
    file.write_all(b"\n").expect("terminate report");
}
