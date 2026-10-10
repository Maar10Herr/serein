//! Small, deterministic query and exact-value contracts used by retrieval.
//!
//! This is deliberately not a general natural-language parser. It recognizes a
//! finite set of answer-type and personal-decision forms, plus exact dates,
//! versions, currencies, unit-bearing quantities, and identifiers. Unknown
//! wording stays `Research`; it never earns a stronger personal attribution.
//! All span offsets are UTF-8 byte offsets into the original input, and `raw`
//! retains the exact source text for display and audit.

use regex::Regex;
use std::{cmp::Reverse, collections::BTreeSet, sync::LazyLock};

const SIGNED_NUMBER: &str =
    r"[+-]?(?:[0-9]{1,3}(?:[,.][0-9]{3})+(?:[,.][0-9]+)?|[0-9]+(?:[.,][0-9]+)?)";

static DATE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9]{4}-[0-9]{2}-[0-9]{2}\b").unwrap());
static VERSION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bv[0-9]+(?:\.[0-9]+)+\b").unwrap());
static MONEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?ix)
        (?P<symbol_sign>[+-])?\s*(?P<symbol>[$€£¥])\s*(?P<symbol_amount>{SIGNED_NUMBER})
        | (?P<prefix_sign>[+-])?\s*(?P<prefix_code>\b(?:USD|EUR|GBP|JPY|CNY)\b)\s*(?P<prefix_amount>{SIGNED_NUMBER})
        | (?P<suffix_amount>{SIGNED_NUMBER})\s*(?P<suffix_code>USD|EUR|GBP|JPY|CNY)\b
        | (?P<suffix_asian_amount>{SIGNED_NUMBER})\s*(?P<suffix_asian_code>円|元)
        "
    ))
    .unwrap()
});
static MONEY_LIKE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)(?:[+-]*\s*[$€£¥]\s*[+-]*[0-9][0-9.,]*|[+-]*\s*\b(?:USD|EUR|GBP|JPY|CNY)\b\s*[+-]*[0-9][0-9.,]*|[+-]?[0-9][0-9.,]*\s*(?:USD|EUR|GBP|JPY|CNY)\b|[+-]?[0-9][0-9.,]*\s*(?:円|元))",
    )
    .unwrap()
});
static MEASUREMENT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)(?P<number>{SIGNED_NUMBER})[\s-]*(?:(?P<unit>mm|cm|kg|MB|Mb|GB|Gb|TB|Tb|MHz|mHz|GHz|W|V|(?i:seconds?|minutes?|hours?|days?|years?|inches|inch))\b|(?P<rate_unit>/\s*(?i:kwh)\b)|(?P<percent>%))"
    ))
    .unwrap()
});
static MEASUREMENT_LIKE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        // This is deliberately case-insensitive only as a malformed-value
        // detector. The exact parser below is case-sensitive for SI symbols
        // whose case changes the quantity (for example GB/Gb and MHz/mHz).
        r"(?ix)[+-]?[0-9][0-9.,]*[\s-]*(?:(?:mm|cm|kg|mb|gb|tb|mhz|ghz|w|v|seconds?|minutes?|hours?|days?|years?|inches|inch)\b|/\s*kwh\b|%)",
    )
    .unwrap()
});
static IDENTIFIER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:usb-[a-z]|[a-z]{1,16}-?[0-9][a-z0-9]*(?:-[a-z0-9]+)*)\b").unwrap()
});
static RANGE_OPERATOR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:<=|>=|≤|≥|<|>|\b(?:under|below|less than|at most|up to|no more than|over|above|more than|at least|no less than|between)\b(?:\s+about\b)?)",
    )
    .unwrap()
});
static RANGE_JOINER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:and|to)\s*").unwrap());
static FROM_RANGE_PREFIX_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bfrom\s*$").unwrap());
static FROM_TO_JOINER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s+to\s*$").unwrap());
// Bounded fail-closed signal only: this recognizes an uppercase three-letter
// suffix in a Money question, not an ISO currency registry or general wording.
static MONEY_CURRENCY_QUALIFIER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?P<qualifier>\bin\s+(?P<code>[A-Z]{3})\b)[?!.]*\s*$").unwrap());
static MONEY_NOUN_QUESTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:what|what's|whats)\s+(?:(?:is|was|are|were)\s+)?(?:the\s+)?(?:price|cost|fee|amount)(?:\s+(?:of|for)\b|[?!.]*\s*$)",
    )
    .unwrap()
});
static MONEY_COST_QUESTION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*what\s+(?:does|do|did)\b.*\bcost(?:s|ing)?\b").unwrap());
static MONEY_ACTION_WORD_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:cost|costs|costing|price|prices|fee|fees|money|pay|paid|spend|spent|charge|charged)\b",
    )
    .unwrap()
});
static QUANTITY_QUESTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\bhow\s+(?:much|many|heavy|large|long|wide|tall)\b|\bwhat\s+(?:is|was)\s+(?:the\s+)?(?:size|weight|mass|length|width|height|distance|duration|time|power|voltage|ram|memory|storage|capacity|frequency)\b)",
    )
    .unwrap()
});
static ENGLISH_DECISION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:did\s+i\s+(?:choose|select|order|pay|spend|buy|purchase|book|reserve|pick|decide)|have\s+i\s+(?:chosen|selected|ordered|paid|spent|bought|purchased|booked|reserved|picked|decided)|i\s+(?:chose|selected|ordered|paid|spent|bought|purchased|booked|reserved|picked|decided))\b",
    )
    .unwrap()
});

/// The kind of answer evidence the question can use. `Research` leaves
/// provenance handling to the caller; `Money` and dimensioned `Quantity`
/// require matching extracted values but do not imply that the user paid or
/// owns them. Currencyless per-kWh rates are measurements, not money.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnswerRequirement {
    Research,
    Money,
    Quantity(QuantityFamily),
}

/// Narrow physical dimensions recognized in finite English quantity forms.
/// Ambiguous requests such as bare "how much", "size", or "capacity" stay
/// `Research` until the question supplies a supported physical dimension.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum QuantityFamily {
    Data,
    Mass,
    Length,
    Time,
    Power,
    Voltage,
    Frequency,
}

/// Query requirements are orthogonal: a query can require money-shaped
/// evidence and a user-confirmed action at the same time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryContract {
    pub answer: AnswerRequirement,
    pub requires_confirmation: bool,
    pub exact_requirements: Vec<ExactRequirement>,
}

/// Canonical categories mirror retrieval's former exact-constraint set so
/// callers can compare `BTreeSet<ExactConstraint>` values without changing
/// their ordering or deduplication rules.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum ExactKind {
    Date,
    Version,
    Currency,
    Measurement,
    Identifier,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExactConstraint {
    pub kind: ExactKind,
    pub normalized: String,
}

/// A parsed number retains its canonical comparison key and whether its
/// punctuation had more than one plausible locale interpretation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExactNumber {
    pub canonical: String,
    pub integer: String,
    pub fraction: Option<String>,
    pub ambiguous_separators: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExactValue {
    Date {
        iso_text: String,
    },
    Version {
        version_text: String,
    },
    Money {
        currency: String,
        amount: ExactNumber,
    },
    Quantity {
        amount: ExactNumber,
        unit: String,
    },
    Identifier {
        normalized: String,
    },
}

/// A complete exact value with byte offsets and the source spelling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExactSpan {
    pub start: usize,
    pub end: usize,
    pub raw: String,
    pub value: ExactValue,
    pub constraint: ExactConstraint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdentifierSpan {
    pub start: usize,
    pub end: usize,
    pub raw: String,
    pub normalized: String,
}

/// The recognized operators remain visible as unsupported rather than being
/// rewritten into equality constraints. `bounds` preserves any nearby exact
/// values but does not assign them range semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnsupportedRange {
    pub operator: String,
    pub start: usize,
    pub end: usize,
    pub raw: String,
    pub bounds: Vec<ExactSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnsupportedValue {
    pub start: usize,
    pub end: usize,
    pub raw: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExactRequirement {
    Equality(ExactSpan),
    UnsupportedRange(UnsupportedRange),
    UnsupportedValue(UnsupportedValue),
}

/// Classify only the query's answer shape and explicit personal decision
/// requirement. Unrecognized queries remain `Research`. The historic
/// multilingual money/decision triggers are retained; new decision grammar is
/// limited to English first-person past forms.
pub(crate) fn analyze_query(query: &str) -> QueryContract {
    let answer = answer_requirement(query);
    let exact_requirements = exact_requirements(query);
    QueryContract {
        answer,
        requires_confirmation: requires_confirmation(query),
        exact_requirements,
    }
}

pub(crate) fn answer_requirement(query: &str) -> AnswerRequirement {
    let lower = query.to_lowercase();
    let has_money_context = has_money_question_trigger(query, &lower);
    let has_money_value = has_explicit_money_trigger(query);

    if has_money_value {
        return AnswerRequirement::Money;
    }

    // An explicit physical target immediately after "how much/many" wins
    // over an incidental spend/pay verb later in a duration question. The
    // same verbs still classify as Money when the target is absent or later.
    if let Some(family) = leading_quantity_family(&lower) {
        return AnswerRequirement::Quantity(family);
    }

    if has_money_context {
        return AnswerRequirement::Money;
    }

    if let Some(family) = quantity_family(&lower) {
        return AnswerRequirement::Quantity(family);
    }

    // Preserve the shipped money guard for its ordinary and multilingual
    // forms, except that recognized quantity targets above take precedence.
    if [
        "exact price",
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
    .any(|phrase| lower.contains(phrase))
    {
        AnswerRequirement::Money
    } else {
        AnswerRequirement::Research
    }
}

fn has_explicit_money_trigger(query: &str) -> bool {
    exact_spans(query)
        .iter()
        .any(|span| matches!(&span.value, ExactValue::Money { .. }))
        || !money_like_spans(query).is_empty()
}

fn has_money_question_trigger(query: &str, lower: &str) -> bool {
    if MONEY_NOUN_QUESTION_PATTERN.is_match(query) || MONEY_COST_QUESTION_PATTERN.is_match(query) {
        return true;
    }
    let Some((_, rest)) = lower.split_once("how much") else {
        return false;
    };
    MONEY_ACTION_WORD_PATTERN.is_match(rest)
}

fn leading_quantity_family(lower: &str) -> Option<QuantityFamily> {
    let trimmed = lower.trim_start();
    let rest = trimmed
        .strip_prefix("how much")
        .or_else(|| trimmed.strip_prefix("how many"))?
        .trim_start();
    let mut words = rest.split(|character: char| !character.is_ascii_alphanumeric());
    let mut target = words.find(|word| !word.is_empty())?;
    if matches!(target, "the" | "a" | "an") {
        target = words.find(|word| !word.is_empty())?;
    }
    match target {
        "ram" | "memory" | "storage" | "bit" | "bits" | "byte" | "bytes" | "mb" | "gb" | "tb" => {
            Some(QuantityFamily::Data)
        }
        "weight" | "mass" | "heavy" | "kg" => Some(QuantityFamily::Mass),
        "length" | "width" | "height" | "distance" | "wide" | "tall" | "mm" | "cm" | "inch"
        | "inches" => Some(QuantityFamily::Length),
        "time" | "duration" | "second" | "seconds" | "minute" | "minutes" | "hour" | "hours"
        | "day" | "days" | "year" | "years" => Some(QuantityFamily::Time),
        "power" | "watt" | "watts" => Some(QuantityFamily::Power),
        "voltage" | "volt" | "volts" => Some(QuantityFamily::Voltage),
        "frequency" | "mhz" | "ghz" => Some(QuantityFamily::Frequency),
        _ => None,
    }
}

fn quantity_family(lower: &str) -> Option<QuantityFamily> {
    if !QUANTITY_QUESTION_PATTERN.is_match(lower) {
        return None;
    }

    let mut families = BTreeSet::new();
    if has_any_word(
        lower,
        &[
            "ram", "memory", "storage", "bit", "bits", "byte", "bytes", "mb", "gb", "tb",
        ],
    ) {
        families.insert(QuantityFamily::Data);
    }
    if has_any_word(lower, &["weight", "mass", "heavy", "weigh", "kg"]) {
        families.insert(QuantityFamily::Mass);
    }
    if has_any_word(
        lower,
        &[
            "length", "width", "height", "distance", "wide", "tall", "mm", "cm", "inch",
        ],
    ) {
        families.insert(QuantityFamily::Length);
    }
    if has_any_word(
        lower,
        &[
            "time", "duration", "minute", "minutes", "hour", "hours", "day", "days", "year",
            "years",
        ],
    ) {
        families.insert(QuantityFamily::Time);
    }
    if has_any_word(lower, &["power"]) {
        families.insert(QuantityFamily::Power);
    }
    if has_any_word(lower, &["voltage"]) {
        families.insert(QuantityFamily::Voltage);
    }
    if has_any_word(lower, &["frequency", "mhz", "ghz"]) {
        families.insert(QuantityFamily::Frequency);
    }

    (families.len() == 1).then(|| *families.iter().next().expect("one family"))
}

fn has_any_word(text: &str, words: &[&str]) -> bool {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| words.contains(&token))
}

fn quantity_family_for_unit(unit: &str) -> Option<QuantityFamily> {
    match unit {
        "gb" | "gb-bit" | "mb" | "mb-bit" | "tb" | "tb-bit" => Some(QuantityFamily::Data),
        "kg" => Some(QuantityFamily::Mass),
        "mm" | "cm" | "inch" => Some(QuantityFamily::Length),
        "second" | "minute" | "hour" | "day" | "year" => Some(QuantityFamily::Time),
        "w" => Some(QuantityFamily::Power),
        "v" => Some(QuantityFamily::Voltage),
        "mhz" | "milli-hz" | "ghz" => Some(QuantityFamily::Frequency),
        // A currencyless energy rate is exact measurement data, but it is not
        // a physical dimension that can answer Money or Time requirements.
        "per-kwh" => None,
        _ => None,
    }
}

/// Existing multilingual confirmation phrases are preserved verbatim. The
/// added English forms are narrow: first-person past/past-participle verbs
/// only, so imperatives and future recommendations remain ordinary research.
pub(crate) fn requires_confirmation(query: &str) -> bool {
    let lower = query.to_lowercase();
    let existing_phrases = [
        "did i choose",
        "did i decide",
        "did i buy",
        "did i purchase",
        "did i book",
        "did i reserve",
        "did i pay",
        "i chose",
        "i decided",
        "i bought",
        "i purchased",
        "i booked",
        "i reserved",
        "i paid",
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
    ];
    existing_phrases.iter().any(|phrase| lower.contains(phrase))
        || lower.contains("did i make") && lower.contains("reservation")
        || ENGLISH_DECISION_PATTERN.is_match(query)
}

/// Extract exact typed spans. Overlapping candidates are resolved in favor of
/// the longest complete value, preserving currencies and version suffixes.
pub(crate) fn exact_spans(text: &str) -> Vec<ExactSpan> {
    let mut found = Vec::<ExactSpan>::new();

    for matched in DATE_PATTERN.find_iter(text) {
        push_span(
            &mut found,
            matched.start(),
            matched.end(),
            text,
            ExactValue::Date {
                iso_text: matched.as_str().to_owned(),
            },
            ExactKind::Date,
            matched.as_str().to_lowercase(),
        );
    }

    for matched in VERSION_PATTERN.find_iter(text) {
        push_span(
            &mut found,
            matched.start(),
            matched.end(),
            text,
            ExactValue::Version {
                version_text: matched.as_str().to_owned(),
            },
            ExactKind::Version,
            matched.as_str().to_lowercase(),
        );
    }

    for captures in MONEY_PATTERN.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        if has_numeric_continuation(text, matched.end())
            || preceding_currency_sign(text, matched.start())
        {
            continue;
        }
        let (currency_text, amount_match, sign) = if let Some(symbol) = captures.name("symbol") {
            (
                symbol.as_str(),
                captures.name("symbol_amount"),
                captures
                    .name("symbol_sign")
                    .map_or("", |value| value.as_str()),
            )
        } else if let Some(currency) = captures.name("prefix_code") {
            (
                currency.as_str(),
                captures.name("prefix_amount"),
                captures
                    .name("prefix_sign")
                    .map_or("", |value| value.as_str()),
            )
        } else {
            (
                captures
                    .name("suffix_code")
                    .or_else(|| captures.name("suffix_asian_code"))
                    .map_or("", |value| value.as_str()),
                captures
                    .name("suffix_amount")
                    .or_else(|| captures.name("suffix_asian_amount")),
                "",
            )
        };
        let Some(amount_match) = amount_match else {
            continue;
        };
        let amount_text = amount_match.as_str();
        if !sign.is_empty()
            && amount_text.starts_with(|character: char| character == '-' || character == '+')
        {
            continue;
        }
        let signed_amount = if sign.is_empty() {
            amount_text.to_owned()
        } else {
            format!("{sign}{amount_text}")
        };
        let currency = canonical_currency(currency_text);
        let amount = parse_exact_number(&signed_amount);
        let normalized = format!("{currency}:{}", amount.canonical);
        push_span(
            &mut found,
            matched.start(),
            matched.end(),
            text,
            ExactValue::Money { currency, amount },
            ExactKind::Currency,
            normalized,
        );
    }

    for captures in MEASUREMENT_PATTERN.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        if has_numeric_continuation(text, matched.end())
            || !has_measurement_start_boundary(text, matched.start())
        {
            continue;
        }
        let Some(number) = captures.name("number") else {
            continue;
        };
        let unit = captures
            .name("unit")
            .map(|value| canonical_unit(value.as_str()))
            .or_else(|| captures.name("rate_unit").map(|_| "per-kwh".to_owned()))
            .unwrap_or_else(|| "%".to_owned());
        let amount = parse_exact_number(number.as_str());
        let normalized = format!("{}:{unit}", amount.canonical);
        push_span(
            &mut found,
            matched.start(),
            matched.end(),
            text,
            ExactValue::Quantity { amount, unit },
            ExactKind::Measurement,
            normalized,
        );
    }

    for identifier in identifier_spans(text) {
        push_span(
            &mut found,
            identifier.start,
            identifier.end,
            text,
            ExactValue::Identifier {
                normalized: identifier.normalized.clone(),
            },
            ExactKind::Identifier,
            identifier.normalized,
        );
    }

    found.sort_by_key(|span| (span.start, Reverse(span.end)));
    let mut non_overlapping = Vec::with_capacity(found.len());
    let mut end = 0;
    for span in found {
        if span.start >= end {
            end = span.end;
            non_overlapping.push(span);
        }
    }
    non_overlapping
}

/// Extract model/connector identifiers separately so comparison parsing can
/// retain its per-arm behavior while sharing exact identifier normalization.
pub(crate) fn identifier_spans(text: &str) -> Vec<IdentifierSpan> {
    IDENTIFIER_PATTERN
        .find_iter(text)
        .map(|matched| IdentifierSpan {
            start: matched.start(),
            end: matched.end(),
            raw: matched.as_str().to_owned(),
            normalized: normalize_identifier(matched.as_str()),
        })
        .collect()
}

/// Return exact and unsupported query constraints. A range bound is reported
/// only as an `UnsupportedRange`; it is deliberately omitted as an equality.
pub(crate) fn exact_requirements(text: &str) -> Vec<ExactRequirement> {
    let spans = exact_spans(text);
    let mut ranges = Vec::new();
    let mut consumed = BTreeSet::<(usize, usize)>::new();

    for operator in RANGE_OPERATOR_PATTERN.find_iter(text) {
        let name = operator.as_str().to_lowercase();
        let Some(first) = spans.iter().find(|span| {
            span.start >= operator.end()
                && text[operator.end()..span.start]
                    .chars()
                    .all(char::is_whitespace)
                && is_range_bound(span)
        }) else {
            continue;
        };
        let bounds = if name == "between" {
            let Some(joiner) = RANGE_JOINER_PATTERN.find(&text[first.end..]) else {
                continue;
            };
            let second_start = first.end + joiner.end();
            let Some(second) = spans.iter().find(|span| {
                span.start >= second_start
                    && text[second_start..span.start]
                        .chars()
                        .all(char::is_whitespace)
                    && is_range_bound(span)
                    && span.constraint.kind == first.constraint.kind
            }) else {
                continue;
            };
            vec![first.clone(), second.clone()]
        } else {
            vec![first.clone()]
        };
        let end = bounds
            .last()
            .map(|bound| bound.end)
            .unwrap_or(operator.end());
        for bound in &bounds {
            consumed.insert((bound.start, bound.end));
        }
        ranges.push(UnsupportedRange {
            operator: name,
            start: operator.start(),
            end,
            raw: text[operator.start()..end].to_owned(),
            bounds,
        });
    }

    // This grammar is deliberately limited to adjacent typed values joined
    // by the explicit "from X to Y" form or a dash. Arbitrary comparison
    // arms and prose between values remain independent exact constraints.
    for pair in spans.windows(2) {
        let [first, second] = pair else {
            continue;
        };
        if !is_range_bound(first)
            || !is_range_bound(second)
            || first.constraint.kind != second.constraint.kind
            || consumed.contains(&(first.start, first.end))
            || consumed.contains(&(second.start, second.end))
        {
            continue;
        }

        let gap = &text[first.end..second.start];
        let prefix = FROM_RANGE_PREFIX_PATTERN.find(&text[..first.start]);
        let from_to = prefix.is_some() && FROM_TO_JOINER_PATTERN.is_match(gap);
        let trimmed_gap = gap.trim();
        let explicit_dash = matches!(trimmed_gap, "-" | "–");
        // The ASCII dash in "€100-€500" is lexically absorbed as the sign
        // of the second value. Treat it as a range divider only when the
        // complete typed spans are otherwise directly adjacent.
        let absorbed_ascii_dash = trimmed_gap.is_empty()
            && first.end == second.start
            && second.raw.trim_start().starts_with('-');

        if !from_to && !explicit_dash && !absorbed_ascii_dash {
            continue;
        }

        let second_bound = if absorbed_ascii_dash {
            unsigned_range_bound(second).unwrap_or_else(|| second.clone())
        } else {
            second.clone()
        };
        let bounds = vec![first.clone(), second_bound];
        let start = prefix.map_or(first.start, |matched| matched.start());
        let end = second.end;
        for bound in [first, second] {
            consumed.insert((bound.start, bound.end));
        }
        ranges.push(UnsupportedRange {
            operator: if from_to { "from/to" } else { "dash" }.to_owned(),
            start,
            end,
            raw: text[start..end].to_owned(),
            bounds,
        });
    }

    let mut invalid_values = malformed_money_spans(text, &spans);
    if let Some(qualifier) = unsupported_currency_qualifier(text, &spans) {
        invalid_values.push(qualifier);
    }
    invalid_values.extend(malformed_measurement_spans(text, &spans));
    let mut requirements = spans
        .into_iter()
        .filter(|span| {
            !consumed.contains(&(span.start, span.end))
                && !invalid_values
                    .iter()
                    .any(|invalid| spans_overlap(invalid.start, invalid.end, span.start, span.end))
        })
        .map(ExactRequirement::Equality)
        .collect::<Vec<_>>();
    requirements.extend(ranges.into_iter().map(ExactRequirement::UnsupportedRange));
    requirements.extend(
        invalid_values
            .into_iter()
            .map(ExactRequirement::UnsupportedValue),
    );
    requirements.sort_by_key(|requirement| match requirement {
        ExactRequirement::Equality(span) => span.start,
        ExactRequirement::UnsupportedRange(range) => range.start,
        ExactRequirement::UnsupportedValue(value) => value.start,
    });
    requirements
}

/// Equality constraints only. Values governed by `under`, `at least`,
/// `between`, and the other recognized range operators are omitted.
pub(crate) fn exact_constraints(text: &str) -> BTreeSet<ExactConstraint> {
    exact_requirements(text)
        .into_iter()
        .filter_map(|requirement| match requirement {
            ExactRequirement::Equality(span) => Some(span.constraint),
            ExactRequirement::UnsupportedRange(_) | ExactRequirement::UnsupportedValue(_) => None,
        })
        .collect()
}

fn unsupported_currency_qualifier(text: &str, parsed: &[ExactSpan]) -> Option<UnsupportedValue> {
    let captures = MONEY_CURRENCY_QUALIFIER_PATTERN.captures(text)?;
    if answer_requirement(text) != AnswerRequirement::Money {
        return None;
    }
    let matched = captures.name("qualifier")?;
    let code = captures.name("code")?.as_str();
    let canonical = canonical_currency(code);
    if parsed.iter().any(
        |span| matches!(&span.value, ExactValue::Money { currency, .. } if *currency == canonical),
    ) {
        return None;
    }
    Some(UnsupportedValue {
        start: matched.start(),
        end: matched.end(),
        raw: text[matched.start()..matched.end()].to_owned(),
        reason: "currency qualifier has no supported exact-amount comparison contract".to_owned(),
    })
}

#[cfg(test)]
pub(crate) fn unsupported_ranges(text: &str) -> Vec<UnsupportedRange> {
    exact_requirements(text)
        .into_iter()
        .filter_map(|requirement| match requirement {
            ExactRequirement::UnsupportedRange(range) => Some(range),
            ExactRequirement::Equality(_) | ExactRequirement::UnsupportedValue(_) => None,
        })
        .collect()
}

/// Test only the required answer shape. Confirmation, suppression, scope, and
/// source identity remain separate retrieval gates.
pub(crate) fn answer_value_supported(requirement: AnswerRequirement, evidence: &str) -> bool {
    match requirement {
        AnswerRequirement::Research => true,
        AnswerRequirement::Money => exact_spans(evidence)
            .iter()
            .any(|span| matches!(&span.value, ExactValue::Money { .. })),
        AnswerRequirement::Quantity(family) => exact_spans(evidence).iter().any(|span| {
            matches!(
                &span.value,
                ExactValue::Quantity { unit, .. }
                    if quantity_family_for_unit(unit) == Some(family)
            )
        }),
    }
}

#[cfg(test)]
pub(crate) fn unsupported_values(text: &str) -> Vec<UnsupportedValue> {
    exact_requirements(text)
        .into_iter()
        .filter_map(|requirement| match requirement {
            ExactRequirement::UnsupportedValue(value) => Some(value),
            ExactRequirement::Equality(_) | ExactRequirement::UnsupportedRange(_) => None,
        })
        .collect()
}

fn push_span(
    output: &mut Vec<ExactSpan>,
    start: usize,
    end: usize,
    text: &str,
    value: ExactValue,
    kind: ExactKind,
    normalized: String,
) {
    output.push(ExactSpan {
        start,
        end,
        raw: text[start..end].to_owned(),
        value,
        constraint: ExactConstraint { kind, normalized },
    });
}

fn is_range_bound(span: &ExactSpan) -> bool {
    matches!(
        &span.value,
        ExactValue::Money { .. } | ExactValue::Quantity { .. }
    )
}

fn unsigned_range_bound(span: &ExactSpan) -> Option<ExactSpan> {
    let sign = span
        .raw
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .filter(|(_, character)| *character == '-')
        .map(|(index, _)| index)?;
    let unsigned_raw = &span.raw[sign + 1..];
    let mut parsed = exact_spans(unsigned_raw)
        .into_iter()
        .find(|candidate| candidate.constraint.kind == span.constraint.kind)?;
    parsed.start = span.start + sign + 1;
    parsed.end = span.end;
    parsed.raw = unsigned_raw.to_owned();
    Some(parsed)
}

fn money_like_spans(text: &str) -> Vec<UnsupportedValue> {
    malformed_money_spans(text, &exact_spans(text))
}

fn malformed_money_spans(text: &str, parsed: &[ExactSpan]) -> Vec<UnsupportedValue> {
    let mut unsupported = Vec::new();
    let mut seen = BTreeSet::new();

    for matched in MONEY_LIKE_PATTERN.find_iter(text) {
        let mut end = matched.end();
        // A final sentence comma or period is outside the numeric token. Keep
        // interior punctuation, which is needed to detect truncated decimals.
        while end > matched.start() {
            let last = text[..end].chars().next_back();
            if matches!(last, Some('.' | ',')) {
                end -= last.expect("matched punctuation").len_utf8();
            } else {
                break;
            }
        }
        if end <= matched.start() {
            continue;
        }
        if parsed.iter().any(|span| {
            matches!(&span.value, ExactValue::Money { .. })
                && span.start == matched.start()
                && span.end == end
        }) {
            continue;
        }
        if !seen.insert((matched.start(), end)) {
            continue;
        }
        unsupported.push(UnsupportedValue {
            start: matched.start(),
            end,
            raw: text[matched.start()..end].to_owned(),
            reason:
                "currency-like value is malformed or cannot be compared as a complete exact amount"
                    .to_owned(),
        });
    }
    unsupported
}

fn malformed_measurement_spans(text: &str, parsed: &[ExactSpan]) -> Vec<UnsupportedValue> {
    let mut unsupported = Vec::new();
    let mut seen = BTreeSet::new();
    for matched in MEASUREMENT_LIKE_PATTERN.find_iter(text) {
        let mut end = matched.end();
        while end > matched.start() {
            let last = text[..end].chars().next_back();
            if matches!(last, Some('.' | ',')) {
                end -= last.expect("matched punctuation").len_utf8();
            } else {
                break;
            }
        }
        if end <= matched.start() {
            continue;
        }
        let overlaps_typed_non_quantity = parsed.iter().any(|span| {
            !matches!(&span.value, ExactValue::Quantity { .. })
                && spans_overlap(matched.start(), end, span.start, span.end)
        });
        if overlaps_typed_non_quantity {
            continue;
        }
        if parsed.iter().any(|span| {
            matches!(&span.value, ExactValue::Quantity { .. })
                && span.start == matched.start()
                && span.end == end
        }) {
            continue;
        }
        if !seen.insert((matched.start(), end)) {
            continue;
        }
        unsupported.push(UnsupportedValue {
            start: matched.start(),
            end,
            raw: text[matched.start()..end].to_owned(),
            reason: "numeric unit-like value is malformed or cannot be compared as a complete exact quantity".to_owned(),
        });
    }
    unsupported
}

fn spans_overlap(start: usize, end: usize, other_start: usize, other_end: usize) -> bool {
    start < other_end && other_start < end
}

fn preceding_currency_sign(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .is_some_and(|character| matches!(character, '+' | '-'))
}

fn has_measurement_start_boundary(text: &str, start: usize) -> bool {
    text[..start].chars().next_back().map_or(true, |character| {
        !character.is_ascii_alphanumeric() && !matches!(character, '.' | ',' | '+' | '-')
    })
}

fn canonical_currency(text: &str) -> String {
    match text.to_lowercase().as_str() {
        "£" | "gbp" => "gbp".to_owned(),
        "€" | "eur" => "eur".to_owned(),
        "usd" => "usd".to_owned(),
        "jpy" | "円" => "jpy".to_owned(),
        "cny" | "元" => "cny".to_owned(),
        "$" => "dollar-symbol".to_owned(),
        "¥" => "yen-yuan-symbol".to_owned(),
        other => other.to_owned(),
    }
}

fn canonical_unit(text: &str) -> String {
    match text {
        "MB" => "mb".to_owned(),
        "Mb" => "mb-bit".to_owned(),
        "GB" => "gb".to_owned(),
        "Gb" => "gb-bit".to_owned(),
        "TB" => "tb".to_owned(),
        "Tb" => "tb-bit".to_owned(),
        "MHz" => "mhz".to_owned(),
        "mHz" => "milli-hz".to_owned(),
        "GHz" => "ghz".to_owned(),
        "W" => "w".to_owned(),
        "V" => "v".to_owned(),
        "second" | "seconds" => "second".to_owned(),
        "minute" | "minutes" => "minute".to_owned(),
        "hour" | "hours" => "hour".to_owned(),
        "day" | "days" => "day".to_owned(),
        "year" | "years" => "year".to_owned(),
        "inch" | "inches" => "inch".to_owned(),
        _ => text.to_lowercase(),
    }
}

fn parse_exact_number(text: &str) -> ExactNumber {
    let raw = text.trim();
    let (sign, magnitude) = if let Some(magnitude) = raw.strip_prefix('-') {
        ("-", magnitude)
    } else if let Some(magnitude) = raw.strip_prefix('+') {
        ("", magnitude)
    } else {
        ("", raw)
    };
    let comma_count = magnitude.matches(',').count();
    let dot_count = magnitude.matches('.').count();
    let has_comma = comma_count > 0;
    let has_dot = dot_count > 0;
    let mut ambiguous = false;

    let (integer, fraction) = if has_comma && has_dot {
        let decimal_mark = if magnitude.rfind(',') > magnitude.rfind('.') {
            ','
        } else {
            '.'
        };
        let Some((whole, fractional)) = magnitude.rsplit_once(decimal_mark) else {
            return literal_number(raw);
        };
        let grouping_mark = if decimal_mark == ',' { '.' } else { ',' };
        if fractional.is_empty() || !valid_grouping(whole, grouping_mark) {
            return literal_number(raw);
        }
        (
            whole.replace(grouping_mark, ""),
            Some(fractional.to_owned()),
        )
    } else if has_comma || has_dot {
        let mark = if has_comma { ',' } else { '.' };
        let count = if has_comma { comma_count } else { dot_count };
        if count > 1 {
            if valid_grouping(magnitude, mark) {
                (magnitude.replace(mark, ""), None)
            } else {
                return literal_number(raw);
            }
        } else {
            let Some((whole, fractional)) = magnitude.split_once(mark) else {
                return literal_number(raw);
            };
            if fractional.len() == 3 && whole.len() <= 3 {
                ambiguous = true;
            } else if fractional.is_empty() {
                return literal_number(raw);
            }
            (whole.to_owned(), Some(fractional.to_owned()))
        }
    } else {
        (magnitude.to_owned(), None)
    };

    if ambiguous {
        return literal_number(raw);
    }

    let integer = trim_leading_zeroes(&integer);
    let fraction = fraction.map(|fraction| trim_trailing_zeroes(&fraction));
    let fraction = fraction.filter(|fraction| !fraction.is_empty());
    let unsigned_canonical = if ambiguous {
        format!("literal:{}", raw.to_lowercase())
    } else if let Some(fraction) = fraction.as_ref() {
        format!("{integer}.{fraction}")
    } else {
        integer.clone()
    };
    let canonical = if sign == "-" {
        if ambiguous {
            format!("literal:{}", raw.to_lowercase())
        } else if unsigned_canonical == "0" {
            "0".to_owned()
        } else {
            format!("-{unsigned_canonical}")
        }
    } else {
        unsigned_canonical
    };
    let integer = if sign == "-" && integer != "0" {
        format!("-{integer}")
    } else {
        integer
    };
    ExactNumber {
        canonical,
        integer,
        fraction,
        ambiguous_separators: ambiguous,
    }
}

fn literal_number(raw: &str) -> ExactNumber {
    ExactNumber {
        canonical: format!("literal:{}", raw.to_lowercase()),
        integer: raw.chars().filter(char::is_ascii_digit).collect(),
        fraction: None,
        ambiguous_separators: true,
    }
}

fn has_numeric_continuation(text: &str, end: usize) -> bool {
    let mut chars = text[end..].chars();
    match chars.next() {
        Some(character) if character.is_ascii_alphanumeric() => true,
        Some(',' | '.') => chars.next().is_some_and(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | ',')
        }),
        _ => false,
    }
}

fn valid_grouping(raw: &str, separator: char) -> bool {
    let parts: Vec<_> = raw.split(separator).collect();
    !parts.is_empty()
        && (1..=3).contains(&parts[0].len())
        && parts[0].chars().all(|character| character.is_ascii_digit())
        && parts[1..]
            .iter()
            .all(|part| part.len() == 3 && part.chars().all(|character| character.is_ascii_digit()))
}

fn trim_leading_zeroes(text: &str) -> String {
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn trim_trailing_zeroes(text: &str) -> String {
    text.trim_end_matches('0').to_owned()
}

fn normalize_identifier(text: &str) -> String {
    let lower = text.to_lowercase();
    let Some(hyphen) = lower.find('-') else {
        return lower;
    };
    let (prefix, rest) = lower.split_at(hyphen);
    let suffix = &rest[1..];
    if !prefix.is_empty()
        && prefix
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        && suffix.starts_with(|character: char| character.is_ascii_digit())
    {
        format!("{prefix}{suffix}")
    } else {
        lower
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_preserves_cents_and_currency_identity() {
        let exact = exact_constraints("$1,200.00 £499 500 EUR €1.200,00");
        assert!(exact.contains(&constraint(ExactKind::Currency, "dollar-symbol:1200")));
        assert!(exact.contains(&constraint(ExactKind::Currency, "gbp:499")));
        assert!(exact.contains(&constraint(ExactKind::Currency, "eur:500")));
        assert!(exact.contains(&constraint(ExactKind::Currency, "eur:1200")));
        assert!(!exact.contains(&constraint(ExactKind::Currency, "dollar-symbol:1200.99")));
    }

    #[test]
    fn ambiguous_currency_symbols_and_separators_stay_literal() {
        let exact = exact_constraints("$1,200 ¥500");
        assert!(exact.contains(&constraint(
            ExactKind::Currency,
            "dollar-symbol:literal:1,200"
        )));
        assert!(exact.contains(&constraint(ExactKind::Currency, "yen-yuan-symbol:500")));
        assert_ne!(exact_constraints("$500"), exact_constraints("USD 500"));
        assert_ne!(exact_constraints("¥500"), exact_constraints("CNY 500"));
        assert_ne!(exact_constraints("$1,200"), exact_constraints("$1200"));
        assert_ne!(exact_constraints("€1.200"), exact_constraints("€1200"));
    }

    #[test]
    fn a_valid_amount_prefix_is_not_taken_from_a_longer_numeric_token() {
        assert!(exact_spans("$1,200.00.99").is_empty());
        assert!(exact_spans("£499abc").is_empty());
        assert!(exact_constraints("$1,200.00.99").is_empty());
        assert!(exact_constraints("£499abc").is_empty());
        assert!(!unsupported_values("$1,200.00.99").is_empty());
        assert!(!unsupported_values("£499abc").is_empty());
    }

    #[test]
    fn signed_money_and_malformed_measurements_never_collapse_to_positive_values() {
        assert_ne!(exact_constraints("$-5"), exact_constraints("$5"));
        assert_ne!(exact_constraints("-€500"), exact_constraints("€500"));
        assert!(
            exact_constraints("$-5").contains(&constraint(ExactKind::Currency, "dollar-symbol:-5"))
        );
        assert!(exact_constraints("-€500").contains(&constraint(ExactKind::Currency, "eur:-500")));

        assert!(exact_spans("1.2.16GB").is_empty());
        assert!(exact_constraints("1.2.16GB").is_empty());
        assert!(!unsupported_values("1.2.16GB").is_empty());
        assert!(exact_spans("a16GB")
            .iter()
            .all(|span| { !matches!(&span.value, ExactValue::Quantity { .. }) }));
        assert!(exact_constraints("16GB").contains(&constraint(ExactKind::Measurement, "16:gb")));
        assert!(exact_constraints("-16GB").contains(&constraint(ExactKind::Measurement, "-16:gb")));
    }

    #[test]
    fn identifiers_keep_connector_suffix_but_allow_model_punctuation_alias() {
        let ids = identifier_spans("USB-C USB-A RX100 RX-100");
        assert_eq!(ids[0].normalized, "usb-c");
        assert_eq!(ids[1].normalized, "usb-a");
        assert_eq!(ids[2].normalized, ids[3].normalized);
        assert_eq!(ids[2].normalized, "rx100");
    }

    #[test]
    fn date_version_and_unit_constraints_keep_their_category_and_value() {
        let exact = exact_constraints("16 GB, 3.5 GHz, 65 W, v2.1 on 2026-09-27");
        assert!(exact.contains(&constraint(ExactKind::Measurement, "16:gb")));
        assert!(exact.contains(&constraint(ExactKind::Measurement, "3.5:ghz")));
        assert!(exact.contains(&constraint(ExactKind::Measurement, "65:w")));
        assert!(exact.contains(&constraint(ExactKind::Version, "v2.1")));
        assert!(exact.contains(&constraint(ExactKind::Date, "2026-09-27")));
    }

    #[test]
    fn range_operators_are_explicit_and_do_not_become_equality() {
        let requirements = exact_requirements("office chair under €500");
        assert_eq!(requirements.len(), 1);
        let ExactRequirement::UnsupportedRange(range) = &requirements[0] else {
            panic!("range must remain unsupported");
        };
        assert_eq!(range.operator, "under");
        assert_eq!(range.raw, "under €500");
        assert_eq!(range.bounds.len(), 1);
        assert!(exact_constraints("office chair under €500").is_empty());

        let pair = exact_requirements("between 16GB and 32GB");
        assert_eq!(pair.len(), 1);
        let ExactRequirement::UnsupportedRange(pair) = &pair[0] else {
            panic!("two adjacent quantity bounds should remain an unsupported range");
        };
        assert_eq!(pair.bounds.len(), 2);
        assert!(exact_constraints("between 16GB and 32GB").is_empty());

        for prose in ["over ear headphones", "switch between Linux and Windows"] {
            assert!(unsupported_ranges(prose).is_empty(), "{prose}");
        }

        for query in ["< €500", ">= 16GB", "≤ 2kg", "under about €500"] {
            let requirements = exact_requirements(query);
            assert!(
                requirements.iter().any(|requirement| matches!(
                    requirement,
                    ExactRequirement::UnsupportedRange(_)
                )),
                "comparison must fail closed: {query}: {requirements:?}"
            );
            assert!(
                exact_constraints(query).is_empty(),
                "range bounds must not be downgraded to equality: {query}"
            );
        }
    }

    #[test]
    fn currency_qualifier_without_an_amount_is_not_ignored() {
        for code in ["CHF", "EUR"] {
            let query = format!("What does the laptop cost in {code}?");
            let requirements = exact_requirements(&query);
            assert!(
                requirements.iter().any(|requirement| matches!(
                    requirement,
                    ExactRequirement::UnsupportedValue(value) if value.raw == format!("in {code}")
                )),
                "currency qualifier must fail closed: {query}: {requirements:?}"
            );
            assert!(exact_constraints(&query).is_empty());
        }

        assert!(exact_constraints("EUR 500").contains(&constraint(ExactKind::Currency, "eur:500")));
    }

    #[test]
    fn adjacent_from_to_and_dash_bounds_are_unsupported_ranges() {
        for query in [
            "from €500 to €1000",
            "from -€500 to €1000",
            "€100–€500",
            "€100-€500",
            "100円-500円",
        ] {
            let requirements = exact_requirements(query);
            assert_eq!(
                requirements.len(),
                1,
                "range should not yield independent equalities: {query}: {requirements:?}"
            );
            let ExactRequirement::UnsupportedRange(range) = &requirements[0] else {
                panic!("adjacent typed bounds must fail closed: {query}: {requirements:?}");
            };
            assert_eq!(range.bounds.len(), 2, "{query}: {range:?}");
            for bound in &range.bounds {
                assert_eq!(query.get(bound.start..bound.end), Some(bound.raw.as_str()));
            }
            assert!(exact_constraints(query).is_empty(), "{query}");
        }

        let negative = exact_requirements("from -€500 to -€1000");
        let ExactRequirement::UnsupportedRange(range) = &negative[0] else {
            panic!("negative endpoints still form an unsupported range: {negative:?}");
        };
        assert_eq!(range.bounds[0].constraint.normalized, "eur:-500");
        assert_eq!(range.bounds[1].constraint.normalized, "eur:-1000");

        for comparison in [
            "compare €500 and €1000",
            "Model A €500 vs Model B €1000",
            "€500; shipping €1000",
        ] {
            let requirements = exact_requirements(comparison);
            assert!(
                requirements.iter().all(|requirement| !matches!(
                    requirement,
                    ExactRequirement::UnsupportedRange(_)
                )),
                "separate values must not be treated as a range: {comparison}: {requirements:?}"
            );
        }
    }

    #[test]
    fn answer_shape_is_narrow_and_currencyless_rates_are_not_money() {
        assert_eq!(
            answer_requirement("cost function optimization research"),
            AnswerRequirement::Research
        );
        assert_eq!(
            answer_requirement("office chair budget research"),
            AnswerRequirement::Research
        );
        assert_eq!(
            answer_requirement("How much does the office chair cost?"),
            AnswerRequirement::Money
        );
        assert_eq!(
            answer_requirement("How much RAM does the laptop have?"),
            AnswerRequirement::Quantity(QuantityFamily::Data)
        );
        assert_eq!(
            answer_requirement("How much does this cost?"),
            AnswerRequirement::Money
        );
        assert_eq!(
            answer_requirement("How much is it?"),
            AnswerRequirement::Research
        );
        assert_eq!(
            answer_requirement("What is the capacity?"),
            AnswerRequirement::Research
        );
        assert_eq!(
            answer_requirement("What is the size?"),
            AnswerRequirement::Research
        );
        assert!(!answer_value_supported(
            AnswerRequirement::Money,
            "rate 0.31/kWh"
        ));
        assert!(exact_constraints("0.31/kWh")
            .contains(&constraint(ExactKind::Measurement, "0.31:per-kwh")));
    }

    #[test]
    fn quantity_money_and_confirmation_are_independent() {
        assert_eq!(
            answer_requirement("How much RAM does the laptop have?"),
            AnswerRequirement::Quantity(QuantityFamily::Data)
        );
        assert_eq!(
            answer_requirement("How much weight is the laptop?"),
            AnswerRequirement::Quantity(QuantityFamily::Mass)
        );
        assert_eq!(
            answer_requirement("How much does the office chair cost?"),
            AnswerRequirement::Money
        );
        let query = analyze_query("How much did I pay for the office chair?");
        assert_eq!(query.answer, AnswerRequirement::Money);
        assert!(query.requires_confirmation);
        let spent = analyze_query("How much did I spend on the office chair?");
        assert_eq!(spent.answer, AnswerRequirement::Money);
        assert!(spent.requires_confirmation);
        assert!(requires_confirmation("I spent £499 on the office chair"));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Data),
            "Laptop with 16GB RAM"
        ));
        assert!(!answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Mass),
            "Laptop with 16GB RAM"
        ));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Mass),
            "Laptop weight is 2kg"
        ));
        assert!(answer_value_supported(
            AnswerRequirement::Money,
            "Chair £499"
        ));
        assert!(!answer_value_supported(
            AnswerRequirement::Money,
            "Laptop 16GB"
        ));
    }

    #[test]
    fn leading_time_target_precedes_spend_while_spending_on_ram_stays_money() {
        let duration = analyze_query("How much time did I spend researching this?");
        assert_eq!(
            duration.answer,
            AnswerRequirement::Quantity(QuantityFamily::Time)
        );
        assert!(duration.requires_confirmation);
        assert_eq!(
            answer_requirement("How much did I spend on RAM?"),
            AnswerRequirement::Money
        );
        assert!(!answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Time),
            "A public listing has USD 500"
        ));
    }

    #[test]
    fn exact_unit_case_preserves_data_and_frequency_dimensions() {
        let exact = exact_constraints("16GB 16Gb 32MB 32Mb 2400MHz 2400mHz");
        for expected in [
            "16:gb",
            "16:gb-bit",
            "32:mb",
            "32:mb-bit",
            "2400:mhz",
            "2400:milli-hz",
        ] {
            assert!(
                exact.contains(&constraint(ExactKind::Measurement, expected)),
                "missing exact unit {expected}: {exact:?}"
            );
        }
        assert_ne!(exact_constraints("16GB"), exact_constraints("16Gb"));
        assert_ne!(exact_constraints("2400MHz"), exact_constraints("2400mHz"));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Data),
            "16GB"
        ));
        // Either is a data-dimension answer to a generic RAM quantity query;
        // the query's exact constraint distinguishes bytes from bits.
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Data),
            "16Gb"
        ));
    }

    #[test]
    fn time_and_other_supported_dimensions_match_only_their_units() {
        assert_eq!(
            answer_requirement("How many minutes does it take?"),
            AnswerRequirement::Quantity(QuantityFamily::Time)
        );
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Time),
            "It takes 12 minutes"
        ));
        assert!(!answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Time),
            "It uses 65W"
        ));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Power),
            "Power draw is 65W"
        ));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Voltage),
            "Voltage is 12V"
        ));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Length),
            "Width is 32mm"
        ));
        assert!(answer_value_supported(
            AnswerRequirement::Quantity(QuantityFamily::Frequency),
            "Clock frequency is 2400MHz"
        ));
    }

    #[test]
    fn finite_money_cost_questions_and_ambiguous_queries_stay_narrow() {
        assert_eq!(
            answer_requirement("What does an office chair cost?"),
            AnswerRequirement::Money
        );
        assert_eq!(
            answer_requirement("What did this chair cost?"),
            AnswerRequirement::Money
        );
        assert_eq!(
            answer_requirement("cost function optimization"),
            AnswerRequirement::Research
        );
        assert_eq!(
            answer_requirement("office chair budget research"),
            AnswerRequirement::Research
        );
        for query in [
            "How much?",
            "How many?",
            "What is the capacity?",
            "What is the size?",
        ] {
            assert_eq!(
                answer_requirement(query),
                AnswerRequirement::Research,
                "ambiguous quantity wording must remain research: {query}"
            );
        }
    }

    #[test]
    fn english_decision_forms_are_narrow_and_research_remains_research() {
        for query in [
            "Which chair did I choose?",
            "Which chair did I select?",
            "Did I order the chair?",
            "I ordered the chair",
        ] {
            assert!(
                requires_confirmation(query),
                "missing confirmation for {query}"
            );
        }
        for query in [
            "Which chair should I select?",
            "Order the chair online",
            "What chair did I research?",
        ] {
            assert!(
                !requires_confirmation(query),
                "unexpected confirmation for {query}"
            );
        }
    }

    fn constraint(kind: ExactKind, normalized: &str) -> ExactConstraint {
        ExactConstraint {
            kind,
            normalized: normalized.to_owned(),
        }
    }
}
