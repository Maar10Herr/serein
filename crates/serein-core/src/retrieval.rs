//! Bounded candidate generation and rank fusion for a local vault.
use crate::{algorithm, model, policy, Recall, Result};
use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
    time::Instant,
};

static EXACT_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
    r"(?ix)
    (?P<date>\b[0-9]{4}-[0-9]{2}-[0-9]{2}\b)
    |(?P<version>\bv[0-9]+(?:\.[0-9]+)+\b)
    |(?P<currency>[$€]\s*[0-9]+(?:[.,][0-9]+)?)
    |(?P<measure>\b[0-9]+(?:[.,][0-9]+)?[\s-]*(?:(?:mm|cm|kg|mb|gb|tb|mhz|ghz|w|v|years?|inches|inch)\b|%))
    |(?P<identifier>\b[a-z]+[0-9][a-z0-9-]*\b)
    "
).expect("fixed exact-constraint pattern")
});

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum ExactKind {
    Date,
    Version,
    Currency,
    Measurement,
    Identifier,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ExactConstraint {
    kind: ExactKind,
    normalized: String,
}

fn exact_constraints(text: &str) -> BTreeSet<ExactConstraint> {
    EXACT_PATTERN
        .captures_iter(text)
        .filter_map(|capture| {
            let (kind, found) = [
                (ExactKind::Date, "date"),
                (ExactKind::Version, "version"),
                (ExactKind::Currency, "currency"),
                (ExactKind::Measurement, "measure"),
                (ExactKind::Identifier, "identifier"),
            ]
            .into_iter()
            .find_map(|(kind, name)| capture.name(name).map(|m| (kind, m.as_str())))?;
            Some(ExactConstraint {
                kind,
                normalized: found
                    .to_lowercase()
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '-')
                    .collect(),
            })
        })
        .collect()
}

fn tokens(request: &Recall) -> Vec<String> {
    const STOP: &[&str] = &[
        "what",
        "about",
        "the",
        "and",
        "with",
        "that",
        "this",
        "does",
        "have",
        "for",
        "are",
        "was",
        "can",
        "how",
        "why",
        "which",
        "when",
        "where",
        "who",
        "you",
        "your",
        "our",
        "its",
        "has",
        "had",
        "did",
        "could",
        "would",
        "should",
        "from",
        "into",
        "than",
        "then",
        "not",
        "but",
        "without",
        "best",
        "topic",
        "intent",
        "constraint",
        "recall",
        "der",
        "die",
        "das",
        "ein",
        "eine",
        "einer",
        "eines",
        "für",
        "dem",
        "den",
        "mit",
        "von",
        "zum",
        "zur",
        "auf",
        "aus",
        "als",
        "welche",
        "welcher",
        "welches",
        "wie",
        "hatte",
        "ich",
        "mein",
        "meine",
        "voor",
        "het",
        "een",
        "dat",
        "wat",
        "welk",
        "had",
        "mijn",
        "met",
        "van",
        "naar",
        "welke",
        "pour",
        "les",
        "des",
        "une",
        "que",
        "quel",
        "quelle",
        "quelles",
        "quels",
        "dans",
        "avec",
        "sans",
        "sur",
        "mon",
        "mes",
        "aux",
        "est",
        "ces",
        "cette",
        "comment",
        "avais",
    ];
    std::iter::once(&request.query)
        .chain(request.facets.iter())
        .flat_map(|s| {
            s.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| {
                    !word.is_empty()
                        && (word.chars().count() > 2
                            || !word.is_ascii()
                            || word.chars().all(|c| c.is_ascii_digit()))
                })
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|word| !STOP.contains(&word.as_str()))
        .take(32)
        .collect()
}

fn rrf(ranks: &[HashMap<String, usize>]) -> Vec<(String, f32)> {
    let mut fused = HashMap::<String, f32>::new();
    for channel in ranks {
        for (id, rank) in channel {
            *fused.entry(id.clone()).or_default() += 1.0 / (algorithm::RRF_OFFSET + rank) as f32;
        }
    }
    let mut fused: Vec<_> = fused.into_iter().collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    fused
}

fn site_filter_sql(policy: &crate::Policy, args: &mut Vec<String>) -> String {
    let mut clauses = Vec::new();
    for site in &policy.excluded_sites {
        clauses.push("(a.site=? OR a.site LIKE ?)".to_string());
        args.push(site.clone());
        args.push(format!("%.{site}"));
    }
    let excluded = if clauses.is_empty() {
        String::new()
    } else {
        format!(" AND NOT ({})", clauses.join(" OR "))
    };
    if !policy.selected_only {
        return excluded;
    }
    let mut selected = Vec::new();
    for site in &policy.selected_sites {
        selected.push("(a.site=? OR a.site LIKE ?)".to_string());
        args.push(site.clone());
        args.push(format!("%.{site}"));
    }
    if selected.is_empty() {
        format!("{excluded} AND 0")
    } else {
        format!("{excluded} AND ({})", selected.join(" OR "))
    }
}

fn lexical_ranks(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    terms: &[String],
) -> Result<HashMap<String, usize>> {
    if terms.is_empty() {
        return Ok(HashMap::new());
    }
    let query = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut args = vec![query, source.to_string()];
    let filter = site_filter_sql(policy, &mut args);
    args.push((algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL as i64).to_string());
    let sql = format!(
        "SELECT a.id FROM atom_fts JOIN atoms a ON a.id=atom_fts.id
WHERE atom_fts MATCH ? AND a.source=?
  {filter}
  AND (a.last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days')
       OR EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action='confirm_constraint'))
ORDER BY bm25(atom_fts),a.id LIMIT ?"
    );
    let ids = conn
        .prepare(&sql)?
        .query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ids
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i + 1))
        .collect())
}

fn semantic_ranks(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    encoder: Option<&model::Encoder>,
    request: &Recall,
    start: Instant,
    budget: u64,
) -> Result<HashMap<String, usize>> {
    let Some(encoder) = encoder else {
        return Ok(HashMap::new());
    };
    let queries: Vec<_> = std::iter::once(&request.query)
        .chain(request.facets.iter())
        .filter_map(|term| encoder.encode(term))
        .collect();
    if queries.is_empty() {
        return Ok(HashMap::new());
    }
    let mut scored = Vec::<(String, f32)>::new();
    let mut args = vec![source.to_string(), encoder.manifest.model_hash.clone()];
    let filter = site_filter_sql(policy, &mut args);
    let sql = format!(
        "SELECT a.id,v.vector FROM vectors v JOIN atoms a ON a.id=v.atom
WHERE a.source=? AND v.model=?
  {filter}
  AND (a.last_seen>=strftime('%Y-%m-%dT%H:%M:%SZ','now','-90 days')
       OR EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action='confirm_constraint'))
ORDER BY a.last_seen DESC,a.id LIMIT 10000"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    for row in rows {
        if start.elapsed().as_millis() as u64 >= budget {
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
        let score = queries
            .iter()
            .map(|query| model::cosine(query, &vector))
            .fold(f32::NEG_INFINITY, f32::max);
        if score.is_finite() && score >= algorithm::SEMANTIC_RECALL_ADMISSION {
            scored.push((id, score));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL);
    Ok(scored
        .into_iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i + 1))
        .collect())
}

fn confirmed_ranks(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    terms: &[String],
) -> Result<HashMap<String, usize>> {
    if terms.is_empty() {
        return Ok(HashMap::new());
    }
    let mut args = vec![source.to_string()];
    let filter = site_filter_sql(policy, &mut args);
    let sql = format!(
        "SELECT f.atom,f.text FROM feedback f JOIN atoms a ON a.id=f.atom
WHERE a.source=? AND f.action='confirm_constraint' AND f.text IS NOT NULL
  {filter}
ORDER BY f.time DESC,f.id DESC LIMIT 1000"
    );
    let rows = conn
        .prepare(&sql)?
        .query_map(params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut latest = HashMap::new();
    for (id, text) in rows {
        latest.entry(id).or_insert(text);
    }
    let mut matches: Vec<_> = latest
        .into_iter()
        .filter_map(|(id, text)| {
            let lower = text.to_lowercase();
            let count = terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            (count > 0).then_some((id, count))
        })
        .collect();
    matches.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    matches.truncate(algorithm::CONFIRMED_FEEDBACK_CANDIDATES);
    Ok(matches
        .into_iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i + 1))
        .collect())
}

/// Return fused candidates. All metadata is fetched after bounded ID generation.
pub fn candidates(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    request: &Recall,
    encoder: Option<&model::Encoder>,
    start: Instant,
    budget: u64,
) -> Result<Vec<(f32, String, Value)>> {
    let terms = tokens(request);
    let exact = exact_constraints(
        &std::iter::once(&request.query)
            .chain(request.facets.iter())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" "),
    );
    let lexical = lexical_ranks(conn, source, policy, &terms)?;
    let semantic = semantic_ranks(conn, source, policy, encoder, request, start, budget)?;
    let confirmed = confirmed_ranks(conn, source, policy, &terms)?;
    let fused = rrf(&[lexical, semantic, confirmed]);
    if fused.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<_> = fused.iter().map(|(id, _)| id.as_str()).collect();
    let placeholders = vec!["?"; ids.len()].join(",");
    let mut metadata = HashMap::new();
    let sql = format!(
        "SELECT id,site,title,query,last_seen FROM atoms WHERE source=? AND id IN ({placeholders})"
    );
    for row in conn.prepare(&sql)?.query_map(
        params_from_iter(std::iter::once(source).chain(ids.iter().copied())),
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    )? {
        let (id, site, title, query, time) = row?;
        metadata.insert(id, (site, title, query, time));
    }
    let mut corrections: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
    let sql = format!(
        "SELECT atom,action,text FROM feedback WHERE atom IN ({placeholders}) ORDER BY time,id"
    );
    for row in conn
        .prepare(&sql)?
        .query_map(params_from_iter(ids.iter().copied()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
    {
        let (id, action, text) = row?;
        corrections.entry(id).or_default().push((action, text));
    }
    let mut sessions = HashMap::<String, i64>::new();
    let sql=format!("SELECT atom,count(DISTINCT session) FROM atom_days WHERE atom IN ({placeholders}) GROUP BY atom");
    for row in conn
        .prepare(&sql)?
        .query_map(params_from_iter(ids.iter().copied()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
    {
        let (id, count) = row?;
        sessions.insert(id, count);
    }
    let mut output = vec![];
    for (id, score) in fused {
        let Some((site, title, query, time)) = metadata.remove(&id) else {
            continue;
        };
        if policy
            .excluded_sites
            .iter()
            .any(|rule| policy::matches(&site, rule))
            || policy.selected_only
                && !policy
                    .selected_sites
                    .iter()
                    .any(|rule| policy::matches(&site, rule))
        {
            continue;
        }
        let corrections = corrections.remove(&id).unwrap_or_default();
        if corrections
            .iter()
            .any(|(action, _)| action == "do_not_use" || action == "wrong_topic")
        {
            continue;
        }
        let confirmed = corrections
            .iter()
            .rev()
            .find(|(action, _)| action == "confirm_constraint");
        if confirmed.is_some() && !request.scope.iter().any(|x| x == "confirmed_preferences")
            || confirmed.is_none()
                && !request
                    .scope
                    .iter()
                    .any(|x| x == "research" || x == "projects")
        {
            continue;
        }
        let raw = confirmed
            .and_then(|(_, text)| text.as_deref())
            .or(query.as_deref())
            .unwrap_or(&title);
        let exact_text = if confirmed.is_some() {
            raw.to_owned()
        } else {
            format!("{title} {}", query.as_deref().unwrap_or(""))
        };
        let available = exact_constraints(&exact_text);
        if !exact
            .iter()
            .all(|requirement| available.contains(requirement))
        {
            continue;
        }
        let subject = if corrections
            .iter()
            .any(|(action, _)| action == "not_about_me")
        {
            "other"
        } else if confirmed.is_some() {
            "self"
        } else {
            "unknown"
        };
        let text = if confirmed.is_some() {
            raw.to_owned()
        } else {
            format!(
                "Observed {} on {}: {}",
                if query.is_some() {
                    "search"
                } else {
                    "page title"
                },
                site,
                raw
            )
        };
        let mut limits = vec![
            "Browsing does not establish endorsement, ownership, or a settled preference."
                .to_string(),
        ];
        for (action, _) in &corrections {
            if action != "confirm_constraint" {
                limits.push(format!("User correction: {}", action.replace('_', " ")));
            }
        }
        output.push((score,site,json!({"id":id,"kind":if confirmed.is_some(){"constraint"}else{"research_topic"},
            "state":if confirmed.is_some(){"confirmed"}else{"observed"},"text":text,"subject":subject,
            "evidence":{"sessions":sessions.get(&id).copied().unwrap_or(0),"sites":1},"last_seen":time,"limits":limits})));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Policy;
    use rusqlite::params;
    #[test]
    fn rrf_candidate_cannot_be_dropped_by_cross_scale_prefusion() {
        let lexical: HashMap<_, _> = (0..96)
            .map(|i| (format!("lexical-{i:03}"), i + 1))
            .collect();
        let semantic = HashMap::from([("semantic-best".to_owned(), 1)]);
        let fused = rrf(&[lexical, semantic]);
        assert!(fused.iter().take(64).any(|(id, _)| id == "semantic-best"));
    }
    #[test]
    fn multi_identifier_query_requires_all_identifiers() {
        let query = exact_constraints("16GB 512GB MacBook");
        assert_eq!(query.len(), 2);
        assert!(!query.is_subset(&exact_constraints("16GB MacBook")));
        assert!(query.is_subset(&exact_constraints("16 GB and 512 GB MacBook")));
    }
    #[test]
    fn exact_constraints_cover_common_units_and_versions() {
        let constraints =
            exact_constraints("16 MB, 3.5 GHz, 65 W, 12 V, 50%, €500, v2.1, 2026-09-27");
        assert_eq!(constraints.len(), 8);
    }
    #[test]
    fn observed_recall_candidate_requires_every_exact_identifier() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE atoms(id TEXT,source TEXT,site TEXT,title TEXT,query TEXT,last_seen TEXT);
CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query);
CREATE TABLE feedback(id TEXT,atom TEXT,action TEXT,text TEXT,time TEXT);
CREATE TABLE atom_days(atom TEXT,session TEXT);") .unwrap();
        for (id, title) in [
            ("partial", "MacBook 16GB"),
            ("complete", "MacBook 16GB 512GB"),
        ] {
            conn.execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?)",
                params![
                    id,
                    "source",
                    "example.com",
                    title,
                    Option::<String>::None,
                    crate::now()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO atom_fts VALUES(?,?,?)",
                params![id, title, Option::<String>::None],
            )
            .unwrap();
            conn.execute("INSERT INTO atom_days VALUES(?,?)", params![id, id])
                .unwrap();
        }
        let request = Recall {
            protocol: 1,
            request_id: crate::id(),
            client: "generic".into(),
            vault: "default".into(),
            query: "16GB 512GB MacBook".into(),
            facets: vec![],
            scope: vec!["research".into()],
            max_bytes: 4096,
            budget_ms: 1500,
        };
        let policy = Policy {
            consent: true,
            recall_enabled: true,
            ..Policy::default()
        };
        let result = candidates(
            &conn,
            "source",
            &policy,
            &request,
            None,
            Instant::now(),
            1500,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].2["id"], "complete");
    }

    #[test]
    fn excluded_sites_do_not_crowd_out_allowed_bm25_hits() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE atoms(id TEXT,source TEXT,site TEXT,title TEXT,last_seen TEXT);
CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query);
CREATE TABLE feedback(atom TEXT,action TEXT);",
        )
        .unwrap();
        for index in 0..80 {
            let id = format!("excluded-{index}");
            conn.execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?)",
                params![
                    id,
                    "source",
                    "excluded.example.com",
                    "keyboard keyboard keyboard",
                    crate::now()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO atom_fts VALUES(?,?,?)",
                params![id, "keyboard keyboard keyboard", Option::<String>::None],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO atoms VALUES(?,?,?,?,?)",
            params![
                "allowed",
                "source",
                "allowed.example.com",
                "keyboard",
                crate::now()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO atom_fts VALUES(?,?,?)",
            params!["allowed", "keyboard", Option::<String>::None],
        )
        .unwrap();
        let policy = Policy {
            excluded_sites: vec!["excluded.example.com".into()],
            selected_only: true,
            selected_sites: vec!["allowed.example.com".into()],
            ..Policy::default()
        };
        let ranks = lexical_ranks(&conn, "source", &policy, &["keyboard".into()]).unwrap();
        assert_eq!(ranks.get("allowed"), Some(&1));
        assert_eq!(ranks.len(), 1);
    }
}
