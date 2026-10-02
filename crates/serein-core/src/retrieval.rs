//! Bounded candidate generation and rank fusion for a local vault.
use crate::feedback::{self, FeedbackEntry, FeedbackResolution};
use crate::{algorithm, importance, model, policy, Recall, Result};
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

#[derive(Clone, Debug)]
struct IdentifierSpan {
    start: usize,
    end: usize,
    raw: String,
    normalized: String,
}

fn standalone_word_spans(text: &str) -> Vec<(String, usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(word_start) = start.take() {
            words.push((text[word_start..index].to_owned(), word_start, index));
        }
    }
    if let Some(word_start) = start {
        words.push((text[word_start..].to_owned(), word_start, text.len()));
    }
    words
}

/// Recognize only the fixed two-identifier comparison form. Each arm gets the
/// shared qualifiers and its own identifier, with facets excluded from the
/// plan so another facet cannot become a hard requirement.
fn comparison_arm_requests(request: &Recall) -> Option<[Recall; 2]> {
    let query = request.query.as_str();
    let words = standalone_word_spans(query);
    let first_word = words.first()?;
    let leading_whitespace = query.len() - query.trim_start().len();
    if !first_word.0.eq_ignore_ascii_case("compare")
        || query[..first_word.1].trim().len() != 0
        || first_word.1 != leading_whitespace
    {
        return None;
    }

    let and_words: Vec<_> = words
        .iter()
        .filter(|(word, _, _)| word.eq_ignore_ascii_case("and"))
        .collect();
    if and_words.len() != 1
        || words
            .iter()
            .any(|(word, _, _)| word.eq_ignore_ascii_case("or"))
    {
        return None;
    }
    let and_word = and_words[0];

    let mut identifiers: Vec<_> = EXACT_PATTERN
        .captures_iter(query)
        .filter_map(|capture| {
            let matched = capture.name("identifier")?;
            let normalized = exact_constraints(matched.as_str())
                .into_iter()
                .find(|constraint| constraint.kind == ExactKind::Identifier)?
                .normalized;
            Some(IdentifierSpan {
                start: matched.start(),
                end: matched.end(),
                raw: matched.as_str().to_owned(),
                normalized,
            })
        })
        .collect();
    if identifiers.len() != 2 {
        return None;
    }
    identifiers.sort_by_key(|identifier| identifier.start);
    if identifiers[0].normalized == identifiers[1].normalized
        || identifiers[0].end >= and_word.1
        || identifiers[1].start <= and_word.2
    {
        return None;
    }

    let mut removals = vec![
        (first_word.1, first_word.2),
        (and_word.1, and_word.2),
        (identifiers[0].start, identifiers[0].end),
        (identifiers[1].start, identifiers[1].end),
    ];
    removals.sort_by(|left, right| right.0.cmp(&left.0));
    let mut shared = query.to_owned();
    for (start, end) in removals {
        shared.replace_range(start..end, " ");
    }
    let shared = shared.split_whitespace().collect::<Vec<_>>().join(" ");
    identifiers.sort_by(|left, right| left.normalized.cmp(&right.normalized));

    let arm_request = |identifier: &IdentifierSpan| {
        let mut arm = request.clone();
        arm.query = if shared.is_empty() {
            identifier.raw.clone()
        } else {
            format!("{shared} {}", identifier.raw)
        };
        arm.facets.clear();
        arm
    };
    Some([arm_request(&identifiers[0]), arm_request(&identifiers[1])])
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OrderedDirection {
    from: String,
    to: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DirectionRelation {
    Aligned,
    Neutral,
    Reversed,
}

fn direction_word_spans(text: &str) -> Vec<(String, usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
            continue;
        }
        if character == '-' && start.is_some() {
            let next_is_alphanumeric = text[index + character.len_utf8()..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
            if next_is_alphanumeric {
                continue;
            }
        }
        if let Some(word_start) = start.take() {
            words.push((text[word_start..index].to_owned(), word_start, index));
        }
    }
    if let Some(word_start) = start {
        words.push((text[word_start..].to_owned(), word_start, text.len()));
    }
    words
}

fn separated_by_whitespace(text: &str) -> bool {
    !text.is_empty() && text.chars().all(char::is_whitespace)
}

fn has_direction_prefix_boundary(text: &str, start: usize) -> bool {
    text[..start].chars().last().is_none_or(|character| {
        character.is_whitespace() || matches!(character, '(' | '[' | '{' | '\'' | '"')
    })
}

/// After the destination, accept only terminal punctuation or a whitespace-
/// delimited fixed auxiliary (`is`, `are`, `was`, `were`, `does`, `can`). This
/// keeps a multiword destination from being shortened to its first word while
/// allowing clauses such as “Windows is not supported”; that clause does not
/// change candidate eligibility.
fn has_unambiguous_direction_suffix(text: &str, start: usize) -> bool {
    let suffix = &text[start..];
    let words = direction_word_spans(suffix);
    let Some((first_word, first_start, _)) = words.first() else {
        return suffix.chars().all(|character| {
            character.is_whitespace()
                || matches!(
                    character,
                    '.' | '?'
                        | '!'
                        | ','
                        | ';'
                        | ':'
                        | ')'
                        | ']'
                        | '}'
                        | '\''
                        | '"'
                        | '…'
                        | '。'
                        | '？'
                        | '！'
                        | '）'
                        | '］'
                        | '｝'
                        | '」'
                        | '』'
                        | '”'
                        | '’'
                        | '»'
                        | '›'
                )
        });
    };

    const CLAUSE_BOUNDARIES: &[&str] = &["is", "are", "was", "were", "does", "can"];
    CLAUSE_BOUNDARIES
        .iter()
        .any(|boundary| first_word.eq_ignore_ascii_case(boundary))
        && separated_by_whitespace(&suffix[..*first_start])
}

/// Recognize one narrow, ordered `from <token> to <token>` phrase. The request
/// caller passes its raw query; candidate classification passes resolved
/// evidence text. Trailing words leave the phrase ambiguous except after a
/// small fixed auxiliary boundary; negation remains ordinary evidence and
/// does not affect eligibility.
fn ordered_direction(text: &str) -> Option<OrderedDirection> {
    let words = direction_word_spans(text);
    let from_positions: Vec<_> = words
        .iter()
        .enumerate()
        .filter_map(|(index, (word, _, _))| word.eq_ignore_ascii_case("from").then_some(index))
        .collect();
    let to_positions: Vec<_> = words
        .iter()
        .enumerate()
        .filter_map(|(index, (word, _, _))| word.eq_ignore_ascii_case("to").then_some(index))
        .collect();
    if from_positions.len() != 1 || to_positions.len() != 1 {
        return None;
    }
    let from_index = from_positions[0];
    let to_index = to_positions[0];
    if from_index + 2 != to_index || to_index + 1 >= words.len() {
        return None;
    }
    let from_span = &words[from_index];
    let from_endpoint = &words[from_index + 1];
    let to_span = &words[to_index];
    let to_endpoint = &words[to_index + 1];
    if !has_direction_prefix_boundary(text, from_span.1)
        || !separated_by_whitespace(&text[from_span.2..from_endpoint.1])
        || !separated_by_whitespace(&text[from_endpoint.2..to_span.1])
        || !separated_by_whitespace(&text[to_span.2..to_endpoint.1])
        || !has_unambiguous_direction_suffix(text, to_endpoint.2)
    {
        return None;
    }
    let from = from_endpoint.0.to_lowercase();
    let to = to_endpoint.0.to_lowercase();
    (from != to).then_some(OrderedDirection { from, to })
}

fn direction_relation(request: &OrderedDirection, evidence_text: &str) -> DirectionRelation {
    let Some(evidence) = ordered_direction(evidence_text) else {
        return DirectionRelation::Neutral;
    };
    if evidence == *request {
        DirectionRelation::Aligned
    } else if evidence.from == request.to && evidence.to == request.from {
        DirectionRelation::Reversed
    } else {
        DirectionRelation::Neutral
    }
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
        "research",
        "researched",
        "look",
        "looked",
        "looking",
        "read",
        "open",
        "opened",
        "visit",
        "visited",
        "saw",
        "page",
        "pages",
        "site",
        "sites",
        "called",
        "compare",
        "comparing",
        "exact",
        "another",
        "session",
        "sessions",
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
        "qué",
        "sobre",
        "investigué",
        "investigue",
        "consulté",
        "leí",
        "abrí",
        "página",
    ];
    std::iter::once(request.query.as_str())
        .chain(request.facets.iter().map(String::as_str))
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

fn compact(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

fn observed_text(site: &str, title: &str, query: Option<&str>) -> String {
    compact(&format!("{site} {title} {}", query.unwrap_or("")))
}

fn main_terms(request: &Recall) -> Vec<String> {
    let mut main = request.clone();
    main.facets.clear();
    tokens(&main)
}

fn deadline_reached(start: Instant, budget: u64) -> bool {
    start.elapsed().as_millis() as u64 >= budget
}

fn admissible_exact_facets(
    request: &Recall,
    encoder: Option<&model::Encoder>,
    start: Instant,
    budget: u64,
) -> Vec<String> {
    if request.facets.is_empty() || deadline_reached(start, budget) {
        return vec![];
    }
    let main_vector = encoder.and_then(|encoder| encoder.encode(&request.query));
    if deadline_reached(start, budget) {
        return vec![];
    }
    let mut admissible = Vec::new();
    for facet in &request.facets {
        if deadline_reached(start, budget) {
            break;
        }
        let words: Vec<_> = facet
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect();
        let non_ascii = facet
            .chars()
            .filter(|ch| ch.is_alphanumeric() && !ch.is_ascii())
            .count();
        let hard_identifier = !exact_constraints(facet).is_empty();
        let specific_phrase =
            compact(facet).chars().count() >= 8 && words.len() >= 2 || non_ascii >= 3;
        if !hard_identifier && !specific_phrase {
            continue;
        }
        if hard_identifier {
            admissible.push(facet.clone());
            continue;
        }
        let Some((encoder, main_vector)) = encoder.zip(main_vector.as_ref()) else {
            continue;
        };
        let Some(facet_vector) = encoder.encode(facet) else {
            if deadline_reached(start, budget) {
                break;
            }
            continue;
        };
        if deadline_reached(start, budget) {
            break;
        }
        if model::cosine(main_vector, &facet_vector) >= algorithm::EXACT_FACET_QUERY_AGREEMENT_FLOOR
        {
            admissible.push(facet.clone());
        }
    }
    admissible
}

fn lexical_coverage(terms: &[String], text: &str) -> (usize, f32) {
    if terms.is_empty() {
        return (0, 0.0);
    }
    let matched = terms
        .iter()
        .filter(|term| {
            let normalized = compact(term);
            text.contains(&normalized)
                || normalized
                    .strip_suffix('s')
                    .is_some_and(|singular| singular.len() >= 4 && text.contains(singular))
                || match normalized.as_str() {
                    "airplane" | "aircraft" => text.contains("plane"),
                    "video" => text.contains("youtube"),
                    _ => false,
                }
        })
        .count();
    (matched, matched as f32 / terms.len() as f32)
}

fn required_acronyms(request: &Recall) -> Vec<String> {
    request
        .query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| {
            part.len() >= 3
                && part.is_ascii()
                && part.chars().any(|ch| ch.is_ascii_alphabetic())
                && part
                    .chars()
                    .all(|ch| !ch.is_ascii_alphabetic() || ch.is_ascii_uppercase())
        })
        .map(compact)
        .collect()
}

fn confirmed_answer_required(query: &str) -> bool {
    let query = query.to_lowercase();
    [
        "did i choose",
        "did i decide",
        "did i buy",
        "did i purchase",
        "did i book",
        "did i reserve",
        "i chose",
        "i decided",
        "i bought",
        "i purchased",
        "i booked",
        "i reserved",
        "i selected",
        "i picked",
        "my preference",
        "my preferences",
        "generally like",
        "strong interest",
        "happened to me",
        "did i prefer",
        "which did i buy",
        "elegí",
        "reservé",
        "prefería",
        "decidí",
        "compré",
        "j'ai choisi",
        "j'ai acheté",
        "j'ai décidé",
        "ik koos",
        "ik heb gekocht",
        "ich habe gekauft",
        "ich entschied",
        "買いました",
        "買った",
        "決めました",
        "決めた",
        "選びました",
        "予約しました",
        "买了",
        "购买了",
        "决定了",
        "选择了",
        "预订了",
        "喜欢",
        "偏好",
    ]
    .iter()
    .any(|phrase| query.contains(phrase))
        || query.contains("did i make") && query.contains("reservation")
}

fn numeric_answer_required(query: &str) -> bool {
    let query = query.to_lowercase();
    [
        "exact price",
        "how much",
        "costaba",
        "cuánto cost",
        "precio exacto",
        "electricity rate",
        "料金はいくら",
        "価格はいくら",
        "多少钱",
        "具体价格",
    ]
    .iter()
    .any(|phrase| query.contains(phrase))
}

static MONEY_VALUE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)(?:[$€¥]\s*\d|\b\d+(?:[.,]\d+)?\s*(?:usd|eur|jpy|円|元|/\s*kwh))")
        .expect("fixed monetary-value pattern")
});

fn bare_identifier_cannot_explain_itself(request: &Recall, raw: &str) -> bool {
    let question = request.query.to_lowercase();
    if ![
        "identify",
        "refer to",
        "stand for",
        "what product",
        "qué producto",
    ]
    .iter()
    .any(|phrase| question.contains(phrase))
    {
        return false;
    }
    let exact = exact_constraints(raw);
    exact.len() == 1
        && exact
            .iter()
            .next()
            .is_some_and(|constraint| constraint.normalized == compact(raw))
}

fn relevance_admitted(
    lexical_rank: Option<usize>,
    semantic_main_score: Option<f32>,
    main_terms: &[String],
    content: &str,
    facets: &[String],
    raw: &str,
) -> bool {
    if main_terms.is_empty() {
        return false;
    }
    let (_, coverage) = lexical_coverage(main_terms, content);
    let semantic_main = semantic_main_score.unwrap_or(0.0);
    let direct_lexical = lexical_rank.is_some() && coverage >= algorithm::DIRECT_LEXICAL_COVERAGE;
    let strong_semantic = semantic_main >= algorithm::SEMANTIC_RECALL_ADMISSION;
    let exact_facet_search = facets.iter().any(|facet| {
        let facet = compact(facet);
        facet.len() >= 3 && compact(raw).contains(&facet)
    });
    direct_lexical || strong_semantic || exact_facet_search
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EvidenceState {
    Observed,
    Confirmed,
}

/// The metadata needed to decide whether an item may enter a retrieval
/// channel. Channel queries resolve state/suppression/retention in SQL and
/// populate this row before counting an accepted rank.
#[derive(Clone, Debug)]
struct CandidateEvidence {
    atom_id: String,
    source_id: String,
    site: String,
    atom_kind: String,
    retained: bool,
    site_allowed: bool,
    recall_suppressed: bool,
    state: EvidenceState,
    latest_confirmation_seq: Option<i64>,
    effective_text: Option<String>,
    original_title: String,
    original_query: Option<String>,
    last_seen: String,
}

impl CandidateEvidence {
    fn observed(
        id: String,
        source: &str,
        site: String,
        kind: String,
        title: String,
        query: Option<String>,
        last_seen: String,
        site_allowed: bool,
    ) -> Self {
        let effective_text = Some(query.as_deref().unwrap_or(&title).to_owned());
        Self {
            atom_id: id,
            source_id: source.to_owned(),
            site,
            atom_kind: kind,
            retained: true,
            site_allowed,
            recall_suppressed: false,
            state: EvidenceState::Observed,
            latest_confirmation_seq: None,
            effective_text,
            original_title: title,
            original_query: query,
            last_seen,
        }
    }

    fn confirmed(
        id: String,
        source: &str,
        site: String,
        kind: String,
        title: String,
        query: Option<String>,
        last_seen: String,
        seq: i64,
        text: Option<String>,
        site_allowed: bool,
    ) -> Self {
        Self {
            atom_id: id,
            source_id: source.to_owned(),
            site,
            atom_kind: kind,
            retained: true,
            site_allowed,
            recall_suppressed: false,
            state: EvidenceState::Confirmed,
            latest_confirmation_seq: Some(seq),
            // Never fall back to the original title/query for confirmed-state
            // relevance. A NULL latest value does not resurrect older text.
            effective_text: text,
            original_title: title,
            original_query: query,
            last_seen,
        }
    }

    fn observed_content(&self) -> String {
        compact(&format!(
            "{} {}",
            self.original_title,
            self.original_query.as_deref().unwrap_or("")
        ))
    }

    fn observed_raw(&self) -> &str {
        self.effective_text.as_deref().unwrap_or("")
    }
}

/// Fully hydrated evidence contract used when formatting a result. Candidate
/// channel rows contain the same resolved state, while correction history is
/// fetched once in a bounded batch after the three channel unions are known.
struct ResolvedEvidence {
    atom_id: String,
    source_id: String,
    site: String,
    #[allow(dead_code)] // Kept in the private evidence contract for policy gates.
    atom_kind: String,
    retained: bool,
    site_allowed: bool,
    recall_suppressed: bool,
    state: EvidenceState,
    latest_confirmation_seq: Option<i64>,
    effective_text: String,
    #[allow(dead_code)]
    original_title: String,
    #[allow(dead_code)]
    original_query: Option<String>,
    correction_actions: Vec<String>,
    subject: String,
    last_seen: String,
}

/// The exact observed payload identity used for packet-level redundancy
/// removal. This intentionally excludes site, atom identity, timestamps and
/// ranking data so equivalent observed payloads from different sites can
/// share one packet slot without merging their stored provenance.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ObservedPayloadKey {
    source_id: String,
    atom_kind: String,
    state: EvidenceState,
    subject: String,
    correction_actions: Vec<String>,
    title: String,
    query: Option<String>,
}

impl ObservedPayloadKey {
    fn from_evidence(evidence: &ResolvedEvidence) -> Self {
        let mut correction_actions: Vec<_> = evidence
            .correction_actions
            .iter()
            .filter(|action| action.as_str() != "confirm_constraint")
            .cloned()
            .collect();
        correction_actions.sort();
        correction_actions.dedup();
        Self {
            source_id: evidence.source_id.clone(),
            atom_kind: evidence.atom_kind.clone(),
            state: evidence.state,
            subject: evidence.subject.clone(),
            correction_actions,
            title: normalize_payload_whitespace(&evidence.original_title),
            query: evidence
                .original_query
                .as_deref()
                .map(normalize_payload_whitespace),
        }
    }
}

fn normalize_payload_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A ranked, fully formatted record together with its real site and the raw
/// structured identity needed by the final packet selector.
#[derive(Clone)]
pub(crate) struct RankedCandidate {
    #[allow(dead_code)] // Retained with the typed candidate for ordered packet policies.
    pub(crate) score: f32,
    pub(crate) site: String,
    pub(crate) record: Value,
    pub(crate) observed_payload_key: Option<ObservedPayloadKey>,
    pub(crate) comparison_arms: u8,
}

pub(crate) struct CandidateBatch {
    pub(crate) candidates: Vec<RankedCandidate>,
    pub(crate) compare_models: bool,
}

impl std::ops::Deref for CandidateBatch {
    type Target = [RankedCandidate];

    fn deref(&self) -> &Self::Target {
        &self.candidates
    }
}

fn format_ranked_candidate(
    score: f32,
    evidence: ResolvedEvidence,
    sessions: i64,
    comparison_arms: u8,
) -> RankedCandidate {
    let confirmed = evidence.state == EvidenceState::Confirmed;
    let raw = evidence.effective_text.as_str();
    let text = if confirmed {
        raw.to_owned()
    } else {
        format!(
            "Observed {} on {}: {}",
            if evidence.original_query.is_some() {
                "search"
            } else {
                "page title"
            },
            evidence.site,
            raw
        )
    };
    let mut limits = vec![
        "Browsing does not establish endorsement, ownership, or a settled preference.".to_owned(),
    ];
    for action in &evidence.correction_actions {
        limits.push(format!("User correction: {}", action.replace('_', " ")));
    }
    let observed_payload_key = (evidence.state == EvidenceState::Observed)
        .then(|| ObservedPayloadKey::from_evidence(&evidence));
    RankedCandidate {
        score,
        site: evidence.site,
        record: json!({"id":evidence.atom_id,"kind":if confirmed{"constraint"}else{"research_topic"},
            "state":if confirmed{"confirmed"}else{"observed"},"text":text,"subject":evidence.subject,
            "evidence":{"sessions":sessions,"sites":1},"last_seen":evidence.last_seen,"limits":limits}),
        observed_payload_key,
        comparison_arms,
    }
}

impl ResolvedEvidence {
    fn hydrate(candidate: CandidateEvidence, feedback: FeedbackResolution) -> Option<Self> {
        let confirmation = feedback.latest_confirmation.as_ref();
        let state = if confirmation.is_some() {
            EvidenceState::Confirmed
        } else {
            EvidenceState::Observed
        };
        let effective_text = match confirmation {
            Some(entry) => entry.text.clone()?,
            None => candidate.effective_text.clone()?,
        };
        Some(Self {
            atom_id: candidate.atom_id,
            source_id: candidate.source_id,
            site: candidate.site,
            atom_kind: candidate.atom_kind,
            retained: candidate.retained,
            site_allowed: candidate.site_allowed,
            recall_suppressed: feedback.recall_suppressed,
            state,
            latest_confirmation_seq: confirmation.map(|entry| entry.seq),
            effective_text,
            original_title: candidate.original_title,
            original_query: candidate.original_query,
            correction_actions: feedback
                .history
                .iter()
                .filter(|entry| entry.action != "confirm_constraint")
                .map(|entry| entry.action.clone())
                .collect(),
            subject: if feedback.has_action("not_about_me") {
                "other".to_owned()
            } else if state == EvidenceState::Confirmed {
                "self".to_owned()
            } else {
                "unknown".to_owned()
            },
            last_seen: candidate.last_seen,
        })
    }
}

struct EligibilityPlan<'a> {
    source: &'a str,
    policy: &'a crate::Policy,
    request: &'a Recall,
    main_terms: Vec<String>,
    exact: BTreeSet<ExactConstraint>,
    exact_facets: Vec<String>,
    acronyms: Vec<String>,
    needs_confirmation: bool,
    needs_number: bool,
    retention_cutoff: String,
    now: i64,
}

impl<'a> EligibilityPlan<'a> {
    fn new(
        source: &'a str,
        policy: &'a crate::Policy,
        request: &'a Recall,
        encoder: Option<&model::Encoder>,
        start: Instant,
        budget: u64,
    ) -> Self {
        Self::new_at(
            source,
            policy,
            request,
            encoder,
            start,
            budget,
            chrono::Utc::now(),
        )
    }

    fn new_at(
        source: &'a str,
        policy: &'a crate::Policy,
        request: &'a Recall,
        encoder: Option<&model::Encoder>,
        start: Instant,
        budget: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let retention_cutoff = (now.clone() - chrono::Duration::days(90))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Self {
            source,
            policy,
            request,
            main_terms: main_terms(request),
            exact: exact_constraints(
                &std::iter::once(&request.query)
                    .chain(request.facets.iter())
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            exact_facets: admissible_exact_facets(request, encoder, start, budget),
            acronyms: required_acronyms(request),
            needs_confirmation: confirmed_answer_required(&request.query),
            needs_number: numeric_answer_required(&request.query),
            retention_cutoff,
            now: now.timestamp(),
        }
    }

    fn site_allowed(&self, site: &str) -> bool {
        !self
            .policy
            .excluded_sites
            .iter()
            .any(|rule| policy::matches(site, rule))
            && (!self.policy.selected_only
                || self
                    .policy
                    .selected_sites
                    .iter()
                    .any(|rule| policy::matches(site, rule)))
    }

    fn in_scope(&self, state: EvidenceState) -> bool {
        match state {
            EvidenceState::Observed => self
                .request
                .scope
                .iter()
                .any(|scope| scope == "research" || scope == "projects"),
            EvidenceState::Confirmed => self
                .request
                .scope
                .iter()
                .any(|scope| scope == "confirmed_preferences"),
        }
    }

    fn exact_supported(&self, text: &str) -> bool {
        let available = exact_constraints(text);
        self.exact
            .iter()
            .all(|required| available.contains(required))
    }

    fn observed_hard_eligible(&self, evidence: &CandidateEvidence) -> bool {
        if evidence.state != EvidenceState::Observed
            || evidence.source_id != self.source
            || !evidence.retained
            || !evidence.site_allowed
            || evidence.recall_suppressed
            || evidence.latest_confirmation_seq.is_some()
            || !self.in_scope(EvidenceState::Observed)
            || self.needs_confirmation
        {
            return false;
        }
        let raw = evidence.observed_raw();
        if (evidence.original_query.is_none()
            && importance::generic_title(&evidence.original_title))
            || evidence.original_query.as_deref().is_some_and(|search| {
                !importance::groupable(&evidence.original_title, Some(search))
                    && exact_constraints(search).is_empty()
            })
            || self.needs_number && !MONEY_VALUE.is_match(raw)
            || bare_identifier_cannot_explain_itself(self.request, raw)
        {
            return false;
        }
        let observed = observed_text(
            &evidence.site,
            &evidence.original_title,
            evidence.original_query.as_deref(),
        );
        self.acronyms
            .iter()
            .all(|acronym| observed.contains(acronym))
            && self.exact_supported(&format!(
                "{} {}",
                evidence.original_title,
                evidence.original_query.as_deref().unwrap_or("")
            ))
    }

    fn observed_supported(
        &self,
        evidence: &CandidateEvidence,
        lexical_rank: Option<usize>,
        semantic_main_score: Option<f32>,
    ) -> bool {
        self.observed_hard_eligible(evidence)
            && relevance_admitted(
                lexical_rank,
                semantic_main_score,
                &self.main_terms,
                &evidence.observed_content(),
                &self.exact_facets,
                evidence.observed_raw(),
            )
    }

    fn confirmed_eligible(&self, evidence: &CandidateEvidence) -> Option<usize> {
        if evidence.state != EvidenceState::Confirmed
            || evidence.source_id != self.source
            || !evidence.retained
            || !evidence.site_allowed
            || evidence.recall_suppressed
            || evidence.latest_confirmation_seq.is_none()
            || !self.in_scope(EvidenceState::Confirmed)
            || self.main_terms.is_empty()
        {
            return None;
        }
        let text = evidence.effective_text.as_deref()?;
        if self.needs_number && !MONEY_VALUE.is_match(text)
            || bare_identifier_cannot_explain_itself(self.request, text)
            || self
                .acronyms
                .iter()
                .any(|acronym| !compact(text).contains(acronym))
            || !self.exact_supported(text)
        {
            return None;
        }
        let (matched, coverage) = lexical_coverage(&self.main_terms, &compact(text));
        (matched > 0 && coverage >= 0.50).then_some(matched)
    }
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

fn semantic_candidate_admitted(main_score: f32) -> bool {
    main_score.is_finite() && main_score >= algorithm::SEMANTIC_RECALL_ADMISSION
}

fn semantic_rank_score(main_score: f32, best_facet_score: Option<f32>) -> f32 {
    let facet_bonus = best_facet_score
        .filter(|score| score.is_finite())
        .map(|score| score.clamp(0.0, 1.0) * algorithm::SEMANTIC_FACET_RANK_BONUS_MAX)
        .unwrap_or(0.0);
    main_score + facet_bonus
}

fn bounded_temporal_rerank(
    mut fused: Vec<(String, f32)>,
    last_seen: &HashMap<String, i64>,
    sessions: &HashMap<String, i64>,
    now: i64,
) -> Vec<(String, f32)> {
    let ceiling = algorithm::RETRIEVAL_TEMPORAL_BONUS_FRACTION / (algorithm::RRF_OFFSET + 1) as f32;
    let log_cap = (1.0 + algorithm::RETRIEVAL_SESSION_CAP as f32).ln();
    for (id, score) in &mut fused {
        let age_days = last_seen.get(id).map_or(90.0, |seen| {
            (now.saturating_sub(*seen).max(0) as f32 / 86_400.0).min(90.0)
        });
        let recency = 2f32.powf(-age_days / algorithm::RETRIEVAL_RECENCY_HALF_LIFE_DAYS);
        let repeat = (1.0
            + sessions
                .get(id)
                .copied()
                .unwrap_or(0)
                .clamp(0, algorithm::RETRIEVAL_SESSION_CAP) as f32)
            .ln()
            / log_cap;
        // Both features lie in [0, 1], so an older high-relevance hit cannot
        // lose to a weak one separated by more than `ceiling` in fused score.
        *score += ceiling
            * (algorithm::RETRIEVAL_RECENCY_SHARE * recency
                + (1.0 - algorithm::RETRIEVAL_RECENCY_SHARE) * repeat);
    }
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

const RETRIEVAL_ATOM_SCAN_LIMIT: usize = 10_000;

fn lexical_ranks(
    conn: &Connection,
    plan: &EligibilityPlan<'_>,
    terms: &[String],
    start: Instant,
    budget: u64,
) -> Result<HashMap<String, usize>> {
    if terms.is_empty() || deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    let query = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut args = vec![
        query,
        plan.source.to_string(),
        plan.retention_cutoff.clone(),
    ];
    let filter = site_filter_sql(plan.policy, &mut args);
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT a.id,a.site,a.kind,a.title,a.query,a.last_seen FROM atom_fts JOIN atoms a ON a.id=atom_fts.id
WHERE atom_fts MATCH ? AND a.source=?
  AND a.last_seen>=?
  {filter}
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action IN ('do_not_use','wrong_topic'))
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action='confirm_constraint')
ORDER BY bm25(atom_fts),a.id LIMIT {RETRIEVAL_ATOM_SCAN_LIMIT}"
    );
    let mut stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    let mut ranks = HashMap::new();
    for row in rows {
        if deadline_reached(start, budget)
            || ranks.len() >= algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL
        {
            break;
        }
        let (id, site, kind, title, query, last_seen) = row?;
        let evidence = CandidateEvidence::observed(
            id.clone(),
            plan.source,
            site.clone(),
            kind,
            title,
            query,
            last_seen,
            plan.site_allowed(&site),
        );
        if plan.observed_supported(&evidence, Some(1), None) {
            ranks.insert(id, ranks.len() + 1);
        }
    }
    Ok(ranks)
}

#[derive(Default)]
struct SemanticMatches {
    ranks: HashMap<String, usize>,
    main_scores: HashMap<String, f32>,
}

fn semantic_ranks(
    conn: &Connection,
    plan: &EligibilityPlan<'_>,
    encoder: Option<&model::Encoder>,
    request: &Recall,
    start: Instant,
    budget: u64,
) -> Result<SemanticMatches> {
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    let Some(encoder) = encoder else {
        return Ok(SemanticMatches::default());
    };
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    let Some(main_query) = encoder.encode(&request.query) else {
        return Ok(SemanticMatches::default());
    };
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    let mut facets = Vec::new();
    for facet in &request.facets {
        if deadline_reached(start, budget) {
            return Ok(SemanticMatches::default());
        }
        if let Some(vector) = encoder.encode(facet) {
            if deadline_reached(start, budget) {
                return Ok(SemanticMatches::default());
            }
            facets.push(vector);
        }
    }
    let mut scored = Vec::<(String, f32, f32)>::new();
    let mut args = vec![
        plan.source.to_string(),
        encoder.manifest.model_hash.clone(),
        plan.retention_cutoff.clone(),
    ];
    let filter = site_filter_sql(plan.policy, &mut args);
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    let sql = format!(
        "SELECT a.id,a.site,a.kind,a.title,a.query,a.last_seen,v.vector FROM vectors v JOIN atoms a ON a.id=v.atom
WHERE a.source=? AND v.model=?
  AND a.last_seen>=?
  {filter}
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action IN ('do_not_use','wrong_topic'))
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action='confirm_constraint')
ORDER BY a.last_seen DESC,a.id LIMIT {RETRIEVAL_ATOM_SCAN_LIMIT}"
    );
    let mut stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    for row in rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (id, site, kind, title, query, last_seen, bytes) = row?;
        if bytes.len() != encoder.manifest.dimensions * 4 {
            continue;
        }
        let evidence = CandidateEvidence::observed(
            id.clone(),
            plan.source,
            site.clone(),
            kind,
            title,
            query,
            last_seen,
            plan.site_allowed(&site),
        );
        if !plan.observed_hard_eligible(&evidence) {
            continue;
        }
        let vector: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|part| f32::from_le_bytes(part.try_into().unwrap()))
            .collect();
        let main = model::cosine(&main_query, &vector);
        let facet = facets
            .iter()
            .map(|query| model::cosine(query, &vector))
            .filter(|score| score.is_finite())
            .reduce(f32::max);
        // The main question is the semantic admission signal. Facets can add a
        // small positive ordering bonus, but a generic or cross-language facet
        // cannot suppress a candidate that fits the question itself.
        if semantic_candidate_admitted(main) && plan.observed_supported(&evidence, None, Some(main))
        {
            scored.push((id, semantic_rank_score(main, facet), main));
        }
    }
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    if deadline_reached(start, budget) {
        return Ok(SemanticMatches::default());
    }
    scored.truncate(algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL);
    let mut matches = SemanticMatches::default();
    for (index, (id, _, main_score)) in scored.into_iter().enumerate() {
        matches.ranks.insert(id.clone(), index + 1);
        matches.main_scores.insert(id, main_score);
    }
    Ok(matches)
}

fn confirmed_ranks(
    conn: &Connection,
    plan: &EligibilityPlan<'_>,
    start: Instant,
    budget: u64,
) -> Result<HashMap<String, usize>> {
    if deadline_reached(start, budget)
        || plan.main_terms.is_empty()
        || !plan.in_scope(EvidenceState::Confirmed)
    {
        return Ok(HashMap::new());
    }
    let mut args = vec![plan.source.to_string()];
    let filter = site_filter_sql(plan.policy, &mut args);
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT f.seq,f.atom,f.text,a.site,a.kind,a.title,a.query,a.last_seen
FROM feedback f JOIN atoms a ON a.id=f.atom
WHERE a.source=? AND f.action='confirm_constraint'
  AND f.seq=(SELECT max(latest.seq) FROM feedback latest
             WHERE latest.atom=f.atom AND latest.action='confirm_constraint')
  AND NOT EXISTS (SELECT 1 FROM feedback suppressed
                  WHERE suppressed.atom=a.id
                    AND suppressed.action IN ('do_not_use','wrong_topic'))
  {filter}
ORDER BY f.seq DESC,a.id LIMIT 1000"
    );
    let mut stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, String>(7)?,
        ))
    })?;
    let mut metadata = HashMap::new();
    let mut latest_rows = Vec::new();
    for row in rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (seq, id, text, site, kind, title, query, last_seen) = row?;
        metadata.insert(id.clone(), (site, kind, title, query, last_seen));
        latest_rows.push(FeedbackEntry {
            seq,
            atom: id,
            action: "confirm_constraint".to_owned(),
            text,
        });
    }
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    // The query uses the sequence index to limit the source to one latest row
    // per atom. Feed those rows through the shared resolver so recall,
    // dashboard, and explain keep a single definition of “latest”.
    let latest = feedback::resolve_rows(latest_rows);
    let mut matches = Vec::new();
    for (id, resolution) in latest {
        if deadline_reached(start, budget) {
            break;
        }
        let Some((site, kind, title, query, last_seen)) = metadata.remove(&id) else {
            continue;
        };
        let Some(confirmation) = resolution.latest_confirmation.as_ref() else {
            continue;
        };
        let evidence = CandidateEvidence::confirmed(
            id.clone(),
            plan.source,
            site.clone(),
            kind,
            title,
            query,
            last_seen,
            confirmation.seq,
            confirmation.text.clone(),
            plan.site_allowed(&site),
        );
        if let Some(matched) = plan.confirmed_eligible(&evidence) {
            matches.push((id, matched));
        }
    }
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    matches.sort_by(|a: &(String, usize), b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if deadline_reached(start, budget) {
        return Ok(HashMap::new());
    }
    matches.truncate(algorithm::CONFIRMED_FEEDBACK_CANDIDATES);
    Ok(matches
        .into_iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i + 1))
        .collect())
}

const COMPARISON_LEXICAL_CANDIDATES_PER_ARM: usize =
    algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL / 2;
const COMPARISON_SEMANTIC_CANDIDATES_PER_ARM: usize =
    algorithm::RETRIEVAL_CANDIDATES_PER_CHANNEL / 2;
const COMPARISON_CONFIRMED_CANDIDATES_PER_ARM: usize = algorithm::CONFIRMED_FEEDBACK_CANDIDATES / 2;

#[derive(Default)]
struct ComparisonSemanticMatches {
    ranks: [HashMap<String, usize>; 2],
    main_scores: [HashMap<String, f32>; 2],
}

#[derive(Default)]
struct ComparisonConfirmedMatches {
    ranks: [HashMap<String, usize>; 2],
    sequences: HashMap<String, i64>,
}

struct MergedArmRanks {
    ranks: HashMap<String, usize>,
    arm_masks: HashMap<String, u8>,
}

fn merge_arm_ranks(arm_ranks: &[HashMap<String, usize>; 2]) -> MergedArmRanks {
    let mut merged = HashMap::<String, (usize, u8)>::new();
    for (arm, ranks) in arm_ranks.iter().enumerate() {
        let bit = 1 << arm;
        for (id, rank) in ranks {
            let entry = merged.entry(id.clone()).or_insert((*rank, 0));
            entry.0 = entry.0.min(*rank);
            entry.1 |= bit;
        }
    }
    let mut ordered: Vec<_> = merged
        .into_iter()
        .map(|(id, (best_rank, arms))| (best_rank, id, arms))
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut ranks = HashMap::with_capacity(ordered.len());
    let mut arm_masks = HashMap::with_capacity(ordered.len());
    for (index, (_, id, arms)) in ordered.into_iter().enumerate() {
        ranks.insert(id.clone(), index + 1);
        arm_masks.insert(id, arms);
    }
    MergedArmRanks { ranks, arm_masks }
}

fn comparison_lexical_ranks(
    conn: &Connection,
    plans: &[EligibilityPlan<'_>; 2],
    start: Instant,
    budget: u64,
) -> Result<[HashMap<String, usize>; 2]> {
    let mut ranks = std::array::from_fn(|_| HashMap::new());
    if deadline_reached(start, budget) {
        return Ok(ranks);
    }
    let terms: BTreeSet<_> = plans
        .iter()
        .flat_map(|plan| plan.main_terms.iter().cloned())
        .collect();
    if terms.is_empty() {
        return Ok(ranks);
    }
    let query = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut args = vec![
        query,
        plans[0].source.to_string(),
        plans[0].retention_cutoff.clone(),
    ];
    let filter = site_filter_sql(plans[0].policy, &mut args);
    if deadline_reached(start, budget) {
        return Ok(ranks);
    }
    let sql = format!(
        "SELECT a.id,a.site,a.kind,a.title,a.query,a.last_seen FROM atom_fts JOIN atoms a ON a.id=atom_fts.id
WHERE atom_fts MATCH ? AND a.source=?
  AND a.last_seen>=?
  {filter}
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action IN ('do_not_use','wrong_topic'))
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action='confirm_constraint')
ORDER BY bm25(atom_fts),a.id LIMIT {RETRIEVAL_ATOM_SCAN_LIMIT}"
    );
    let mut stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(ranks);
    }
    let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        if deadline_reached(start, budget)
            || ranks
                .iter()
                .all(|arm| arm.len() >= COMPARISON_LEXICAL_CANDIDATES_PER_ARM)
        {
            break;
        }
        let (id, site, kind, title, query, last_seen) = row?;
        let evidence = CandidateEvidence::observed(
            id.clone(),
            plans[0].source,
            site.clone(),
            kind,
            title,
            query,
            last_seen,
            plans[0].site_allowed(&site),
        );
        for arm in 0..2 {
            if ranks[arm].len() >= COMPARISON_LEXICAL_CANDIDATES_PER_ARM {
                continue;
            }
            if plans[arm].observed_supported(&evidence, Some(1), None) {
                let next_rank = ranks[arm].len() + 1;
                ranks[arm].insert(id.clone(), next_rank);
            }
        }
    }
    Ok(ranks)
}

fn comparison_semantic_ranks(
    conn: &Connection,
    plans: &[EligibilityPlan<'_>; 2],
    encoder: Option<&model::Encoder>,
    start: Instant,
    budget: u64,
) -> Result<ComparisonSemanticMatches> {
    let mut matches = ComparisonSemanticMatches::default();
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let Some(encoder) = encoder else {
        return Ok(matches);
    };
    let Some(query_a) = encoder.encode(&plans[0].request.query) else {
        return Ok(matches);
    };
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let Some(query_b) = encoder.encode(&plans[1].request.query) else {
        return Ok(matches);
    };
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let query_vectors = [query_a, query_b];
    let mut scored: [Vec<(String, f32)>; 2] = std::array::from_fn(|_| Vec::new());
    let mut args = vec![
        plans[0].source.to_owned(),
        encoder.manifest.model_hash.clone(),
        plans[0].retention_cutoff.clone(),
    ];
    let filter = site_filter_sql(plans[0].policy, &mut args);
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let sql = format!(
        "SELECT a.id,a.site,a.kind,a.title,a.query,a.last_seen,v.vector FROM vectors v JOIN atoms a ON a.id=v.atom
WHERE a.source=? AND v.model=?
  AND a.last_seen>=?
  {filter}
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action IN ('do_not_use','wrong_topic'))
  AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.atom=a.id AND f.action='confirm_constraint')
ORDER BY a.last_seen DESC,a.id LIMIT {RETRIEVAL_ATOM_SCAN_LIMIT}"
    );
    let mut stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    for row in rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (id, site, kind, title, query, last_seen, bytes) = row?;
        if bytes.len() != encoder.manifest.dimensions * 4 {
            continue;
        }
        let evidence = CandidateEvidence::observed(
            id.clone(),
            plans[0].source,
            site.clone(),
            kind,
            title,
            query,
            last_seen,
            plans[0].site_allowed(&site),
        );
        if !plans
            .iter()
            .any(|plan| plan.observed_hard_eligible(&evidence))
        {
            continue;
        }
        let vector: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|part| f32::from_le_bytes(part.try_into().unwrap()))
            .collect();
        for arm in 0..2 {
            let main = model::cosine(&query_vectors[arm], &vector);
            if semantic_candidate_admitted(main)
                && plans[arm].observed_supported(&evidence, None, Some(main))
            {
                scored[arm].push((id.clone(), main));
            }
        }
    }
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    for arm in 0..2 {
        scored[arm].sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
        if deadline_reached(start, budget) {
            return Ok(ComparisonSemanticMatches::default());
        }
        scored[arm].truncate(COMPARISON_SEMANTIC_CANDIDATES_PER_ARM);
        for (index, (id, score)) in scored[arm].drain(..).enumerate() {
            matches.ranks[arm].insert(id.clone(), index + 1);
            matches.main_scores[arm].insert(id, score);
        }
    }
    Ok(matches)
}

fn comparison_confirmed_ranks(
    conn: &Connection,
    plans: &[EligibilityPlan<'_>; 2],
    start: Instant,
    budget: u64,
) -> Result<ComparisonConfirmedMatches> {
    let mut matches = ComparisonConfirmedMatches::default();
    if deadline_reached(start, budget)
        || plans
            .iter()
            .all(|plan| plan.main_terms.is_empty() || !plan.in_scope(EvidenceState::Confirmed))
    {
        return Ok(matches);
    }
    let mut args = vec![plans[0].source.to_owned()];
    let filter = site_filter_sql(plans[0].policy, &mut args);
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let sql = format!(
        "SELECT f.seq,f.atom,f.text,a.site,a.kind,a.title,a.query,a.last_seen
FROM feedback f JOIN atoms a ON a.id=f.atom
WHERE a.source=? AND f.action='confirm_constraint'
  AND f.seq=(SELECT max(latest.seq) FROM feedback latest
             WHERE latest.atom=f.atom AND latest.action='confirm_constraint')
  AND NOT EXISTS (SELECT 1 FROM feedback suppressed
                  WHERE suppressed.atom=a.id
                    AND suppressed.action IN ('do_not_use','wrong_topic'))
  {filter}
ORDER BY f.seq DESC,a.id LIMIT 1000"
    );
    let mut stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(matches);
    }
    let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?,
        ))
    })?;
    let mut ranked: [Vec<(String, usize, i64)>; 2] = std::array::from_fn(|_| Vec::new());
    for row in rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (seq, id, text, site, kind, title, query, last_seen) = row?;
        let evidence = CandidateEvidence::confirmed(
            id.clone(),
            plans[0].source,
            site.clone(),
            kind,
            title,
            query,
            last_seen,
            seq,
            text,
            plans[0].site_allowed(&site),
        );
        for arm in 0..2 {
            if let Some(matched) = plans[arm].confirmed_eligible(&evidence) {
                ranked[arm].push((id.clone(), matched, seq));
            }
        }
    }
    if deadline_reached(start, budget) {
        return Ok(ComparisonConfirmedMatches::default());
    }
    for arm in 0..2 {
        ranked[arm].sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        if deadline_reached(start, budget) {
            return Ok(ComparisonConfirmedMatches::default());
        }
        ranked[arm].truncate(COMPARISON_CONFIRMED_CANDIDATES_PER_ARM);
        for (index, (id, _, seq)) in ranked[arm].drain(..).enumerate() {
            matches.ranks[arm].insert(id.clone(), index + 1);
            matches.sequences.insert(id, seq);
        }
    }
    Ok(matches)
}

fn comparison_candidates(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    arm_requests: &[Recall; 2],
    encoder: Option<&model::Encoder>,
    start: Instant,
    budget: u64,
    variant: RankingVariant,
) -> Result<Vec<RankedCandidate>> {
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let now = chrono::Utc::now();
    let plans = [
        EligibilityPlan::new_at(
            source,
            policy,
            &arm_requests[0],
            encoder,
            start,
            budget,
            now.clone(),
        ),
        EligibilityPlan::new_at(
            source,
            policy,
            &arm_requests[1],
            encoder,
            start,
            budget,
            now,
        ),
    ];
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }

    // Each channel traverses the shared eligible source once, evaluates both
    // arm plans during that traversal, and applies the fixed per-arm slots
    // before merging. All work uses the one original request clock/deadline.
    let lexical_by_arm = comparison_lexical_ranks(conn, &plans, start, budget)?;
    let semantic_by_arm = comparison_semantic_ranks(conn, &plans, encoder, start, budget)?;
    let confirmed_by_arm = comparison_confirmed_ranks(conn, &plans, start, budget)?;
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let lexical = merge_arm_ranks(&lexical_by_arm);
    let semantic = merge_arm_ranks(&semantic_by_arm.ranks);
    let confirmed = merge_arm_ranks(&confirmed_by_arm.ranks);
    let mut supported_arms = HashMap::<String, u8>::new();
    for channel in [&lexical, &semantic, &confirmed] {
        for (id, arms) in &channel.arm_masks {
            *supported_arms.entry(id.clone()).or_default() |= *arms;
        }
    }
    let mut fused = rrf(&[
        lexical.ranks.clone(),
        semantic.ranks.clone(),
        confirmed.ranks.clone(),
    ]);
    if fused.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<_> = fused.iter().map(|(id, _)| id.as_str()).collect();
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT id,site,kind,title,query,last_seen FROM atoms
WHERE source=? AND id IN ({placeholders})
  AND (last_seen>=? OR EXISTS (SELECT 1 FROM feedback f
                               WHERE f.atom=atoms.id AND f.action='confirm_constraint'))"
    );
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let mut metadata_stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let metadata_rows = metadata_stmt.query_map(
        params_from_iter(
            std::iter::once(source)
                .chain(ids.iter().copied())
                .chain(std::iter::once(plans[0].retention_cutoff.as_str())),
        ),
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )?;
    let mut metadata = HashMap::new();
    for row in metadata_rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (id, site, kind, title, query, last_seen) = row?;
        metadata.insert(id, (site, kind, title, query, last_seen));
    }
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let feedback_ids: Vec<_> = ids.iter().map(|id| (*id).to_owned()).collect();
    let mut feedback_by_atom = feedback::load_for_atoms(conn, &feedback_ids)?;
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let mut sessions = HashMap::<String, i64>::new();
    let sessions_sql = format!(
        "SELECT atom,count(DISTINCT session) FROM atom_days WHERE atom IN ({placeholders}) GROUP BY atom"
    );
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let mut sessions_stmt = conn.prepare(&sessions_sql)?;
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    let sessions_rows = sessions_stmt.query_map(params_from_iter(ids.iter().copied()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in sessions_rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (id, count) = row?;
        sessions.insert(id, count);
    }
    if deadline_reached(start, budget) {
        return Ok(Vec::new());
    }
    if variant == RankingVariant::BoundedTemporal {
        let last_seen: HashMap<String, i64> = metadata
            .iter()
            .filter_map(|(id, (_, _, _, _, time))| {
                chrono::DateTime::parse_from_rfc3339(time)
                    .ok()
                    .map(|parsed| (id.clone(), parsed.timestamp()))
            })
            .collect();
        fused = bounded_temporal_rerank(fused, &last_seen, &sessions, plans[0].now);
    }

    let mut output = Vec::new();
    for (id, score) in fused {
        if deadline_reached(start, budget) {
            break;
        }
        let Some((site, kind, title, query, last_seen)) = metadata.remove(&id) else {
            continue;
        };
        let feedback = feedback_by_atom.remove(&id).unwrap_or_default();
        let site_allowed = plans[0].site_allowed(&site);
        let candidate = if let Some(confirmation) = feedback.latest_confirmation.as_ref() {
            CandidateEvidence::confirmed(
                id.clone(),
                source,
                site,
                kind,
                title,
                query,
                last_seen,
                confirmation.seq,
                confirmation.text.clone(),
                site_allowed,
            )
        } else {
            CandidateEvidence::observed(
                id.clone(),
                source,
                site,
                kind,
                title,
                query,
                last_seen,
                site_allowed,
            )
        };
        let expected_confirmation = confirmed_by_arm.sequences.get(&id).copied();
        let mut candidate = candidate;
        candidate.recall_suppressed = feedback.recall_suppressed;
        let Some(evidence) = ResolvedEvidence::hydrate(candidate.clone(), feedback) else {
            continue;
        };
        let confirmation_sequence_matches = if evidence.state == EvidenceState::Confirmed {
            expected_confirmation == evidence.latest_confirmation_seq
        } else {
            expected_confirmation.is_none()
        };
        if evidence.source_id != source
            || !evidence.retained
            || !evidence.site_allowed
            || evidence.recall_suppressed
            || !confirmation_sequence_matches
        {
            continue;
        }

        let channel_arms = supported_arms.get(&id).copied().unwrap_or_default();
        let mut candidate_arms = 0u8;
        for arm in 0..2 {
            let arm_bit = 1 << arm;
            if channel_arms & arm_bit == 0 {
                continue;
            }
            let eligible = match evidence.state {
                EvidenceState::Observed => plans[arm].observed_supported(
                    &candidate,
                    lexical_by_arm[arm].get(&id).copied(),
                    semantic_by_arm.main_scores[arm].get(&id).copied(),
                ),
                EvidenceState::Confirmed => {
                    confirmed_by_arm.ranks[arm].contains_key(&id)
                        && plans[arm].confirmed_eligible(&candidate).is_some()
                }
            };
            if eligible {
                candidate_arms |= arm_bit;
            }
        }
        if candidate_arms == 0 {
            continue;
        }
        output.push(format_ranked_candidate(
            score,
            evidence,
            sessions.get(&id).copied().unwrap_or_default(),
            candidate_arms,
        ));
    }
    Ok(output)
}

/// Return fused candidates. All metadata is fetched after bounded ID generation.
pub(crate) fn candidates(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    request: &Recall,
    encoder: Option<&model::Encoder>,
    start: Instant,
    budget: u64,
) -> Result<CandidateBatch> {
    if let Some(arms) = comparison_arm_requests(request) {
        return Ok(CandidateBatch {
            candidates: comparison_candidates(
                conn,
                source,
                policy,
                &arms,
                encoder,
                start,
                budget,
                configured_variant(),
            )?,
            compare_models: true,
        });
    }
    Ok(CandidateBatch {
        candidates: candidates_with_variant(
            conn,
            source,
            policy,
            request,
            encoder,
            start,
            budget,
            configured_variant(),
        )?,
        compare_models: false,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RankingVariant {
    Baseline,
    BoundedTemporal,
}

fn configured_variant() -> RankingVariant {
    match algorithm::RETRIEVAL_RANKING_VERSION {
        1 => RankingVariant::Baseline,
        3 => RankingVariant::BoundedTemporal,
        version => panic!("unsupported retrieval ranking version {version}"),
    }
}

fn candidates_with_variant(
    conn: &Connection,
    source: &str,
    policy: &crate::Policy,
    request: &Recall,
    encoder: Option<&model::Encoder>,
    start: Instant,
    budget: u64,
    variant: RankingVariant,
) -> Result<Vec<RankedCandidate>> {
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let terms = tokens(request);
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let plan = EligibilityPlan::new(source, policy, request, encoder, start, budget);
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let lexical = if deadline_reached(start, budget) {
        HashMap::new()
    } else {
        lexical_ranks(conn, &plan, &terms, start, budget)?
    };
    let semantic = if deadline_reached(start, budget) {
        SemanticMatches::default()
    } else {
        semantic_ranks(conn, &plan, encoder, request, start, budget)?
    };
    let confirmed = if deadline_reached(start, budget) {
        HashMap::new()
    } else {
        confirmed_ranks(conn, &plan, start, budget)?
    };
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let mut fused = rrf(&[lexical.clone(), semantic.ranks.clone(), confirmed]);
    if fused.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<_> = fused.iter().map(|(id, _)| id.as_str()).collect();
    let placeholders = vec!["?"; ids.len()].join(",");
    let mut metadata = HashMap::new();
    let sql = format!(
        "SELECT id,site,kind,title,query,last_seen FROM atoms
WHERE source=? AND id IN ({placeholders})
  AND (last_seen>=? OR EXISTS (SELECT 1 FROM feedback f
                               WHERE f.atom=atoms.id AND f.action='confirm_constraint'))"
    );
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let mut metadata_stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let metadata_rows = metadata_stmt.query_map(
        params_from_iter(
            std::iter::once(source)
                .chain(ids.iter().copied())
                .chain(std::iter::once(plan.retention_cutoff.as_str())),
        ),
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
            ))
        },
    )?;
    for row in metadata_rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (id, site, kind, title, query, time) = row?;
        metadata.insert(id, (site, kind, title, query, time));
    }
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let feedback_ids: Vec<_> = ids.iter().map(|id| (*id).to_string()).collect();
    let mut feedback_by_atom = feedback::load_for_atoms(conn, &feedback_ids)?;
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let mut sessions = HashMap::<String, i64>::new();
    let sql=format!("SELECT atom,count(DISTINCT session) FROM atom_days WHERE atom IN ({placeholders}) GROUP BY atom");
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let mut sessions_stmt = conn.prepare(&sql)?;
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    let sessions_rows = sessions_stmt.query_map(params_from_iter(ids.iter().copied()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in sessions_rows {
        if deadline_reached(start, budget) {
            break;
        }
        let (id, count) = row?;
        sessions.insert(id, count);
    }
    if deadline_reached(start, budget) {
        return Ok(vec![]);
    }
    if variant == RankingVariant::BoundedTemporal {
        let last_seen: HashMap<String, i64> = metadata
            .iter()
            .filter_map(|(id, (_, _, _, _, time))| {
                chrono::DateTime::parse_from_rfc3339(time)
                    .ok()
                    .map(|parsed| (id.clone(), parsed.timestamp()))
            })
            .collect();
        fused = bounded_temporal_rerank(fused, &last_seen, &sessions, plan.now);
    }
    let direction = ordered_direction(&request.query);
    let mut output = vec![];
    let mut direction_ranked = Vec::<(DirectionRelation, f32, String, RankedCandidate)>::new();
    for (id, score) in fused {
        if deadline_reached(start, budget) {
            break;
        }
        let Some((site, kind, title, query, time)) = metadata.remove(&id) else {
            continue;
        };
        let feedback = feedback_by_atom.remove(&id).unwrap_or_default();
        let site_allowed = plan.site_allowed(&site);
        let candidate = if let Some(confirmation) = feedback.latest_confirmation.as_ref() {
            CandidateEvidence::confirmed(
                id.clone(),
                source,
                site,
                kind,
                title,
                query,
                time,
                confirmation.seq,
                confirmation.text.clone(),
                site_allowed,
            )
        } else {
            CandidateEvidence::observed(
                id.clone(),
                source,
                site,
                kind,
                title,
                query,
                time,
                site_allowed,
            )
        };
        let mut candidate = candidate;
        candidate.recall_suppressed = feedback.recall_suppressed;
        let Some(evidence) = ResolvedEvidence::hydrate(candidate.clone(), feedback) else {
            continue;
        };
        let eligible = match evidence.state {
            EvidenceState::Observed => plan.observed_supported(
                &candidate,
                lexical.get(&id).copied(),
                semantic.main_scores.get(&id).copied(),
            ),
            EvidenceState::Confirmed => plan.confirmed_eligible(&candidate).is_some(),
        };
        let resolved_sequence_matches =
            evidence.latest_confirmation_seq == candidate.latest_confirmation_seq;
        if evidence.source_id != source
            || !evidence.retained
            || !evidence.site_allowed
            || evidence.recall_suppressed
            || !resolved_sequence_matches
            || !eligible
        {
            continue;
        }
        let relation = direction
            .as_ref()
            .map(|direction| direction_relation(direction, &evidence.effective_text));
        let ranked_candidate =
            format_ranked_candidate(score, evidence, sessions.get(&id).copied().unwrap_or(0), 0);
        if let Some(relation) = relation {
            direction_ranked.push((relation, score, id, ranked_candidate));
        } else {
            output.push(ranked_candidate);
        }
    }
    if direction.is_some() {
        direction_ranked.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| right.1.total_cmp(&left.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        output.extend(
            direction_ranked
                .into_iter()
                .map(|(_, _, _, candidate)| candidate),
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Policy;
    use rusqlite::params;

    fn recall(query: &str, facets: &[&str]) -> Recall {
        Recall {
            protocol: 1,
            request_id: crate::id(),
            client: "generic".into(),
            vault: "default".into(),
            query: query.into(),
            facets: facets.iter().map(|facet| (*facet).to_string()).collect(),
            scope: vec!["research".into()],
            max_bytes: 4096,
            budget_ms: 1500,
        }
    }

    #[test]
    fn comparison_parser_canonicalizes_only_the_explicit_two_identifier_form() {
        let normal = comparison_arm_requests(&recall(
            "Compare Sony RX100 and ZV1 16GB cameras USB-C",
            &["ignored facet ZV1"],
        ))
        .expect("explicit two-model comparison parses");
        assert_eq!(normal[0].query, "Sony 16GB cameras USB-C RX100");
        assert_eq!(normal[1].query, "Sony 16GB cameras USB-C ZV1");
        assert!(normal.iter().all(|arm| arm.facets.is_empty()));
        assert_eq!(
            exact_constraints(&normal[0].query),
            BTreeSet::from([
                ExactConstraint {
                    kind: ExactKind::Identifier,
                    normalized: "rx100".into(),
                },
                ExactConstraint {
                    kind: ExactKind::Measurement,
                    normalized: "16gb".into(),
                },
            ])
        );

        let swapped = comparison_arm_requests(&recall(
            " compare sony zv1 and rx100 16GB cameras USB-C ",
            &[],
        ))
        .expect("lowercase and swapped wording still parses");
        assert_eq!(swapped[0].query, "sony 16GB cameras USB-C rx100");
        assert_eq!(swapped[1].query, "sony 16GB cameras USB-C zv1");

        for unsupported in [
            "What about Sony RX100 and ZV1 cameras",
            "Compare Sony RX100 or ZV1 cameras",
            "Compare Sony RX100 and ZV1 and A6700 cameras",
            "Compare RX100 and RX100 cameras",
            "Compare v2.1 and v2.2 software",
            "Compare 16GB and 512GB MacBook models",
        ] {
            assert!(
                comparison_arm_requests(&recall(unsupported, &[])).is_none(),
                "unsupported comparison form was activated: {unsupported}"
            );
        }
    }

    #[test]
    fn ordered_direction_accepts_one_unicode_hyphenated_pair_only() {
        let parsed = ordered_direction("Weekend travel from Åland-2 to 東京").unwrap();
        assert_eq!(parsed.from, "åland-2");
        assert_eq!(parsed.to, "東京");
        assert_eq!(
            ordered_direction("Not from north to south.").unwrap(),
            OrderedDirection {
                from: "north".into(),
                to: "south".into(),
            }
        );
        let linux_to_windows = ordered_direction("migration from Linux to Windows").unwrap();
        assert_eq!(
            direction_relation(
                &linux_to_windows,
                "migration from Linux to Windows is not supported"
            ),
            DirectionRelation::Aligned
        );

        for unsupported in [
            "travel from New York to Berlin",
            "travel from Berlin to New York",
            "travel from north to south from east to west",
            "travel from north to south to east",
            "travel from north to North",
            "travel from A to B/C",
            "travel from A to B_C",
            "travel from A to B+",
            "travel from A/B to C",
            "travel from A_B to C",
            "travel from A+ to C",
            "travel from north to south tomorrow",
            "travel_from north to south",
            "buy apple, not buy pear",
            "shipping-from warehouse to customer",
            "shipping from warehouse to-customer",
        ] {
            assert!(
                ordered_direction(unsupported).is_none(),
                "unsupported directional form was activated: {unsupported}"
            );
        }
    }

    #[test]
    fn comparison_mode_precedes_ordered_direction() {
        let request = recall("Compare Alpha1 and Beta1 from north to south", &[]);
        let conn = Connection::open_in_memory().unwrap();
        let batch = candidates(
            &conn,
            "source",
            &Policy::default(),
            &request,
            None,
            Instant::now(),
            0,
        )
        .unwrap();
        assert!(batch.compare_models);
        assert!(batch.candidates.is_empty());
    }

    #[test]
    fn comparison_channel_reservations_and_expired_start_are_bounded() {
        assert_eq!(COMPARISON_LEXICAL_CANDIDATES_PER_ARM, 32);
        assert_eq!(COMPARISON_SEMANTIC_CANDIDATES_PER_ARM, 32);
        assert_eq!(COMPARISON_CONFIRMED_CANDIDATES_PER_ARM, 16);

        // An expired request must return before preparing any channel query.
        // The connection intentionally has no schema, so touching SQLite would
        // turn this regression into an error instead of an empty candidate set.
        let conn = Connection::open_in_memory().unwrap();
        let policy = Policy {
            consent: true,
            recall_enabled: true,
            ..Policy::default()
        };
        let arms = comparison_arm_requests(&recall("Compare RX100 and ZV1 cameras", &[]))
            .expect("fixed comparison form");
        let expired = Instant::now() - std::time::Duration::from_secs(1);
        let candidates = comparison_candidates(
            &conn,
            "source",
            &policy,
            &arms,
            None,
            expired,
            1,
            RankingVariant::BoundedTemporal,
        )
        .unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn comparison_joint_hit_has_one_rank_and_one_vote_per_channel() {
        let first_arm = HashMap::from([("joint-page".to_owned(), 1)]);
        let second_arm = HashMap::from([("joint-page".to_owned(), 1)]);
        let merged = merge_arm_ranks(&[first_arm, second_arm]);
        assert_eq!(merged.ranks.len(), 1);
        assert_eq!(merged.ranks["joint-page"], 1);
        assert_eq!(merged.arm_masks["joint-page"], 0b11);

        let fused = rrf(&[merged.ranks.clone(), merged.ranks.clone()]);
        assert_eq!(fused.len(), 1);
        assert_eq!(
            fused[0].1,
            2.0 / (algorithm::RRF_OFFSET + 1) as f32,
            "the joint atom contributes once to each channel, not once per arm"
        );
    }

    fn retrieval_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE atoms(id TEXT,source TEXT,site TEXT,title TEXT,query TEXT,kind TEXT,last_seen TEXT);
CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query);
CREATE TABLE feedback(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,atom TEXT,action TEXT,text TEXT,time TEXT);
CREATE TABLE atom_days(atom TEXT,session TEXT);",
        )
        .unwrap();
        conn
    }

    fn insert_observation(conn: &Connection, id: &str, title: &str, last_seen: &str) {
        conn.execute(
            "INSERT INTO atoms VALUES(?,?,?,?,?,?,?)",
            params![
                id,
                "source",
                "docs.example",
                title,
                Option::<String>::None,
                "visit",
                last_seen
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
    fn semantic_facets_only_add_a_bounded_positive_bonus() {
        let main = 0.61;
        let no_facet = semantic_rank_score(main, None);
        assert!(semantic_candidate_admitted(main));
        assert!(semantic_candidate_admitted(
            algorithm::SEMANTIC_RECALL_ADMISSION
        ));
        assert!(!semantic_candidate_admitted(
            algorithm::SEMANTIC_RECALL_ADMISSION - 0.001
        ));
        assert!(!semantic_candidate_admitted(f32::NAN));

        assert_eq!(semantic_rank_score(main, Some(-0.4)), no_facet);
        let with_facet = semantic_rank_score(main, Some(0.9));
        assert!(with_facet > no_facet);
        assert!(with_facet <= main + algorithm::SEMANTIC_FACET_RANK_BONUS_MAX);
        assert!(
            semantic_rank_score(0.67, None) > semantic_rank_score(main, Some(1.0)),
            "a facet bonus must not override a larger main-question score gap"
        );
    }

    #[test]
    fn semantic_answerability_requires_main_terms_and_supported_evidence() {
        let main_terms = vec!["quiet".to_string(), "grinder".to_string()];
        assert!(relevance_admitted(
            None,
            Some(algorithm::SEMANTIC_RECALL_ADMISSION),
            &main_terms,
            "manual coffee mill",
            &[],
            "manual coffee mill",
        ));
        assert!(!relevance_admitted(
            None,
            Some(algorithm::SEMANTIC_RECALL_ADMISSION - 0.001),
            &main_terms,
            "manual coffee mill",
            &[],
            "manual coffee mill",
        ));
        assert!(!relevance_admitted(
            None,
            Some(1.0),
            &[],
            "manual coffee mill",
            &["manual coffee mill".to_string()],
            "manual coffee mill",
        ));
        assert!(relevance_admitted(
            Some(1),
            None,
            &["grinder".to_string()],
            "manual grinder",
            &[],
            "manual grinder",
        ));
    }

    #[test]
    fn exact_facet_support_requires_the_saved_text_to_contain_the_facet() {
        let main_terms = vec!["train".to_string(), "schedule".to_string()];
        let exact_facet = vec!["ThinkPad X1 Carbon".to_string()];
        assert!(relevance_admitted(
            None,
            Some(0.1),
            &main_terms,
            "thinkpad x1 carbon laptop",
            &exact_facet,
            "ThinkPad X1 Carbon",
        ));
        assert!(!relevance_admitted(
            None,
            Some(0.1),
            &main_terms,
            "train schedule",
            &exact_facet,
            "Train schedule",
        ));
    }

    #[test]
    fn temporal_variant_preserves_admitted_candidate_eligibility() {
        let conn = retrieval_conn();
        insert_observation(
            &conn,
            "query-hit",
            "Office chair and monitor setup",
            &crate::now(),
        );
        insert_observation(
            &conn,
            "facet-only",
            "Office chair ergonomic posture guide",
            &crate::now(),
        );
        let request = recall("office chair setup", &["ergonomic"]);
        let policy = Policy {
            consent: true,
            recall_enabled: true,
            ..Policy::default()
        };

        let baseline = candidates_with_variant(
            &conn,
            "source",
            &policy,
            &request,
            None,
            Instant::now(),
            1500,
            RankingVariant::Baseline,
        )
        .unwrap();
        let proposed = candidates_with_variant(
            &conn,
            "source",
            &policy,
            &request,
            None,
            Instant::now(),
            1500,
            RankingVariant::BoundedTemporal,
        )
        .unwrap();
        let baseline_ids: BTreeSet<_> = baseline
            .iter()
            .map(|candidate| candidate.record["id"].as_str().unwrap())
            .collect();
        let proposed_ids: BTreeSet<_> = proposed
            .iter()
            .map(|candidate| candidate.record["id"].as_str().unwrap())
            .collect();
        assert!(baseline_ids.contains("facet-only"));
        assert_eq!(proposed_ids, baseline_ids);
    }

    #[test]
    fn generic_home_title_does_not_become_assistant_context() {
        let conn = retrieval_conn();
        insert_observation(&conn, "feed", "Home / X", &crate::now());
        let request = recall("What was on the home page?", &[]);
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
        assert!(result.is_empty());
    }

    #[test]
    fn facet_match_without_main_question_support_abstains() {
        let conn = retrieval_conn();
        insert_observation(
            &conn,
            "unrelated-chair",
            "Office chair buying guide",
            &crate::now(),
        );
        let request = recall("What did I research about trains?", &["chair"]);
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
        assert!(result.is_empty());

        conn.execute(
            "UPDATE atoms SET site='trains.example' WHERE id='unrelated-chair'",
            [],
        )
        .unwrap();
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
        assert!(
            result.is_empty(),
            "hostname-only overlap must not satisfy the facet gate"
        );

        let weak_main = recall(
            "What did I research about chair ergonomics budget?",
            &["buying guide"],
        );
        let result = candidates(
            &conn,
            "source",
            &policy,
            &weak_main,
            None,
            Instant::now(),
            1500,
        )
        .unwrap();
        assert!(
            result.is_empty(),
            "one matching term must not bypass a multi-term question"
        );
    }

    #[test]
    fn unsupported_choice_and_named_entity_questions_abstain() {
        let conn = retrieval_conn();
        insert_observation(&conn, "chair", "Ergonomic office chair", &crate::now());
        let policy = Policy {
            consent: true,
            recall_enabled: true,
            ..Policy::default()
        };
        for question in [
            "Which ergonomic office chair did I choose?",
            "Did I research an IKEA ergonomic office chair?",
        ] {
            let result = candidates(
                &conn,
                "source",
                &policy,
                &recall(question, &[]),
                None,
                Instant::now(),
                1500,
            )
            .unwrap();
            assert!(result.is_empty(), "unsupported question: {question}");
        }
    }

    #[test]
    fn temporal_bonus_reorders_ties_without_overriding_clear_relevance() {
        let recency_candidates = vec![("older".to_string(), 1.0), ("newer".to_string(), 1.0)];
        let timestamps = HashMap::from([
            ("older".to_string(), 1_767_225_600),
            ("newer".to_string(), 1_790_784_000),
        ]);
        let sessions = HashMap::from([("older".to_string(), 4), ("newer".to_string(), 1)]);
        let reranked =
            bounded_temporal_rerank(recency_candidates, &timestamps, &sessions, 1_790_784_000);
        assert_eq!(reranked[0].0, "newer");
        assert_eq!(
            reranked
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["newer", "older"])
        );

        let session_candidates = vec![
            ("one-session".to_string(), 1.0),
            ("many-sessions".to_string(), 1.0),
        ];
        let same_timestamp = HashMap::from([
            ("one-session".to_string(), 1_790_784_000),
            ("many-sessions".to_string(), 1_790_784_000),
        ]);
        let session_counts = HashMap::from([
            ("one-session".to_string(), 1),
            ("many-sessions".to_string(), 4),
        ]);
        let reranked = bounded_temporal_rerank(
            session_candidates,
            &same_timestamp,
            &session_counts,
            1_790_784_000,
        );
        assert_eq!(reranked[0].0, "many-sessions");

        let clear_relevance = vec![("older".to_string(), 0.020), ("newer".to_string(), 0.015)];
        let reranked =
            bounded_temporal_rerank(clear_relevance, &timestamps, &sessions, 1_790_784_000);
        assert_eq!(reranked[0].0, "older");
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
        conn.execute_batch("CREATE TABLE atoms(id TEXT,source TEXT,site TEXT,title TEXT,query TEXT,kind TEXT,last_seen TEXT);
CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query);
CREATE TABLE feedback(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,atom TEXT,action TEXT,text TEXT,time TEXT);
CREATE TABLE atom_days(atom TEXT,session TEXT);") .unwrap();
        for (id, title) in [
            ("partial", "MacBook 16GB"),
            ("complete", "MacBook 16GB 512GB"),
        ] {
            conn.execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?)",
                params![
                    id,
                    "source",
                    "example.com",
                    title,
                    Option::<String>::None,
                    "visit",
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
        assert_eq!(result[0].record["id"], "complete");
    }

    #[test]
    fn excluded_sites_do_not_crowd_out_allowed_bm25_hits() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE atoms(id TEXT,source TEXT,site TEXT,title TEXT,query TEXT,kind TEXT,last_seen TEXT);
CREATE VIRTUAL TABLE atom_fts USING fts5(id UNINDEXED,title,query);
CREATE TABLE feedback(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,atom TEXT,action TEXT,text TEXT,time TEXT);",
        )
        .unwrap();
        for index in 0..80 {
            let id = format!("excluded-{index}");
            conn.execute(
                "INSERT INTO atoms VALUES(?,?,?,?,?,?,?)",
                params![
                    id,
                    "source",
                    "excluded.example.com",
                    "keyboard overview guide",
                    Option::<String>::None,
                    "visit",
                    crate::now()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO atom_fts VALUES(?,?,?)",
                params![id, "keyboard overview guide", Option::<String>::None],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO atoms VALUES(?,?,?,?,?,?,?)",
            params![
                "allowed",
                "source",
                "allowed.example.com",
                "keyboard overview guide",
                Option::<String>::None,
                "visit",
                crate::now()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO atom_fts VALUES(?,?,?)",
            params!["allowed", "keyboard overview guide", Option::<String>::None],
        )
        .unwrap();
        let policy = Policy {
            excluded_sites: vec!["excluded.example.com".into()],
            selected_only: true,
            selected_sites: vec!["allowed.example.com".into()],
            ..Policy::default()
        };
        let request = recall("keyboard", &[]);
        let start = Instant::now();
        let plan = EligibilityPlan::new("source", &policy, &request, None, start, 1500);
        let ranks = lexical_ranks(&conn, &plan, &["keyboard".into()], start, 1500).unwrap();
        assert_eq!(ranks.get("allowed"), Some(&1));
        assert_eq!(ranks.len(), 1);
    }

    #[test]
    fn semantic_channel_fills_slots_after_suppressed_and_out_of_scope_rows() {
        let conn = retrieval_conn();
        conn.execute_batch(
            "CREATE TABLE vectors(atom TEXT PRIMARY KEY,model TEXT NOT NULL,vector BLOB NOT NULL);",
        )
        .unwrap();
        let model_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/serein-context/runtime/macos-arm64/model");
        let encoder = model::Encoder::open(&model_path).expect("checked-in model pack");
        let query_vector = encoder.encode("Desk lamp installation").unwrap();

        let orthogonal_index = query_vector
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .map(|(index, _)| index)
            .unwrap();
        let mut orthogonal = vec![0.0; query_vector.len()];
        orthogonal[orthogonal_index] = 1.0;
        let projection = query_vector[orthogonal_index];
        for (component, query_component) in orthogonal.iter_mut().zip(&query_vector) {
            *component -= projection * query_component;
        }
        let orthogonal = model::normalize(orthogonal).unwrap();
        let guide_vector: Vec<f32> = query_vector
            .iter()
            .zip(&orthogonal)
            .map(|(query, orthogonal)| query * 0.8 + orthogonal * 0.6)
            .collect();
        let encode_vector = |vector: &[f32]| {
            vector
                .iter()
                .flat_map(|component| component.to_le_bytes())
                .collect::<Vec<_>>()
        };

        for (index, action) in (0..66).map(|index| {
            let action = match index % 3 {
                0 => "do_not_use",
                1 => "wrong_topic",
                _ => "confirm_constraint",
            };
            (index, action)
        }) {
            let id = format!("excluded-{index:02}");
            insert_observation(
                &conn,
                &id,
                &format!("Desk lamp installation reference {index}"),
                &crate::now(),
            );
            conn.execute(
                "INSERT INTO vectors(atom,model,vector) VALUES(?,?,?)",
                params![
                    id,
                    encoder.manifest.model_hash,
                    encode_vector(&query_vector)
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feedback(id,atom,action,text,time) VALUES(?,?,?,?,?)",
                params![
                    format!("feedback-{index}"),
                    id,
                    action,
                    (action == "confirm_constraint").then_some("Desk lamp installation reference"),
                    crate::now()
                ],
            )
            .unwrap();
        }
        insert_observation(
            &conn,
            "eligible-guide",
            "Desk lamp installation guide",
            &crate::now(),
        );
        conn.execute(
            "INSERT INTO vectors(atom,model,vector) VALUES(?,?,?)",
            params![
                "eligible-guide",
                encoder.manifest.model_hash,
                encode_vector(&guide_vector)
            ],
        )
        .unwrap();

        let request = recall("Desk lamp installation", &[]);
        let policy = Policy::default();
        let start = Instant::now();
        let plan = EligibilityPlan::new("source", &policy, &request, Some(&encoder), start, 1500);
        let matches = semantic_ranks(&conn, &plan, Some(&encoder), &request, start, 1500).unwrap();
        assert_eq!(matches.ranks.get("eligible-guide"), Some(&1));
        assert_eq!(matches.ranks.len(), 1);
    }

    #[test]
    fn latest_null_confirmation_does_not_fall_back_to_older_matching_text() {
        let conn = retrieval_conn();
        insert_observation(&conn, "latest-null", "Desk lamp observation", &crate::now());
        insert_observation(
            &conn,
            "latest-valid",
            "Office equipment note",
            &crate::now(),
        );
        conn.execute(
            "INSERT INTO feedback(id,atom,action,text,time) VALUES(?,?,?,?,?)",
            params![
                "old-confirmation",
                "latest-null",
                "confirm_constraint",
                "Desk lamp is the selected option",
                crate::now()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feedback(id,atom,action,text,time) VALUES(?,?,?,?,?)",
            params![
                "new-null-confirmation",
                "latest-null",
                "confirm_constraint",
                Option::<String>::None,
                crate::now()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feedback(id,atom,action,text,time) VALUES(?,?,?,?,?)",
            params![
                "valid-confirmation",
                "latest-valid",
                "confirm_constraint",
                "Desk lamp is preferred",
                crate::now()
            ],
        )
        .unwrap();

        let mut request = recall("Desk lamp", &[]);
        request.scope = vec!["confirmed_preferences".to_owned()];
        let policy = Policy::default();
        let start = Instant::now();
        let plan = EligibilityPlan::new("source", &policy, &request, None, start, 1500);
        let ranks = confirmed_ranks(&conn, &plan, start, 1500).unwrap();
        assert!(!ranks.contains_key("latest-null"));
        assert_eq!(ranks.get("latest-valid"), Some(&1));
    }

    #[test]
    fn expired_shared_deadline_skips_plan_and_channel_queries() {
        let conn = Connection::open_in_memory().unwrap();
        let policy = Policy::default();
        let request = recall("Desk lamp installation", &["specific lamp fixture"]);
        let start = Instant::now() - std::time::Duration::from_millis(10);
        let result = candidates_with_variant(
            &conn,
            "source",
            &policy,
            &request,
            None,
            start,
            1,
            RankingVariant::BoundedTemporal,
        )
        .unwrap();
        assert!(result.is_empty());
    }
}
