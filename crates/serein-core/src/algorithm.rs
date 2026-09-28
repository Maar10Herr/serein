//! Canonical, build-time algorithm parameters.
//! These are calibration values, not probabilities or user preferences.
pub const DIMENSIONS: usize = 256;
pub const QUERY_WEIGHT: f32 = 0.7;
pub const TITLE_WEIGHT: f32 = 0.3;
pub const TOPIC_ADMISSION: f32 = 0.62;
pub const TOPIC_MARGIN: f32 = 0.08;
pub const RUNNER_UP_ADMISSION: f32 = 0.55;
/// Minimum main-question cosine for semantic admission. The 0.45 floor admits
/// multilingual paraphrases below the older 0.55 threshold while retaining a
/// margin over weak generic-topic matches in the development fixtures.
pub const SEMANTIC_RECALL_ADMISSION: f32 = 0.45;
/// Facets may help order query-supported semantic candidates, but never veto them.
pub(crate) const SEMANTIC_FACET_RANK_BONUS_MAX: f32 = 0.05;
/// Direct lexical evidence must cover at least this share of the substantive question terms.
pub(crate) const DIRECT_LEXICAL_COVERAGE: f32 = 0.50;
/// An exact facet can rescue an entity phrase when the model finds no negative
/// relationship between that phrase and the main question.
pub(crate) const EXACT_FACET_QUERY_AGREEMENT_FLOOR: f32 = 0.0;
pub const MAX_TOPICS: usize = 128;
pub const MAX_CONTEXT_RECORDS: usize = 6;
pub const MAX_RECORDS_PER_SITE: u32 = 2;
pub const ACTIVITY_HALF_LIVES_DAYS: [f64; 3] = [1.0, 7.0, 30.0];
pub const ACTIVITY_PRIOR_STRENGTH: f64 = 2.0;
pub const BURST_DAILY_RATE_PRIOR: f64 = 0.01;
pub const RRF_OFFSET: usize = 60;
pub const RETRIEVAL_CANDIDATES_PER_CHANNEL: usize = 64;
pub const CONFIRMED_FEEDBACK_CANDIDATES: usize = 32;
/// Retrieval v3 keeps ordinal lexical/semantic fusion and admits evidence
/// before a bounded temporal ordering bonus.
pub(crate) const RETRIEVAL_RANKING_VERSION: u32 = 3;
/// The maximum temporal bonus is this fraction of one rank-one RRF channel.
pub(crate) const RETRIEVAL_TEMPORAL_BONUS_FRACTION: f32 = 0.1;
pub(crate) const RETRIEVAL_RECENCY_HALF_LIFE_DAYS: f32 = 14.0;
pub(crate) const RETRIEVAL_SESSION_CAP: i64 = 4;
pub(crate) const RETRIEVAL_RECENCY_SHARE: f32 = 0.7;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibration_values_have_valid_relationships() {
        assert_eq!(DIMENSIONS, 256);
        assert!((QUERY_WEIGHT + TITLE_WEIGHT - 1.0).abs() < 1e-6);
        assert!(TOPIC_ADMISSION > RUNNER_UP_ADMISSION);
        assert!(TOPIC_MARGIN > 0.0 && TOPIC_MARGIN < TOPIC_ADMISSION);
        assert!(SEMANTIC_RECALL_ADMISSION > 0.0 && SEMANTIC_RECALL_ADMISSION < 1.0);
        assert!((0.0..=0.10).contains(&SEMANTIC_FACET_RANK_BONUS_MAX));
        assert!((0.0..=1.0).contains(&DIRECT_LEXICAL_COVERAGE));
        assert!((-1.0..=1.0).contains(&EXACT_FACET_QUERY_AGREEMENT_FLOOR));
        assert!(MAX_TOPICS >= MAX_CONTEXT_RECORDS);
        assert!(MAX_RECORDS_PER_SITE as usize <= MAX_CONTEXT_RECORDS);
        assert!(ACTIVITY_HALF_LIVES_DAYS
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(ACTIVITY_PRIOR_STRENGTH > 0.0 && BURST_DAILY_RATE_PRIOR > 0.0);
        assert!(RRF_OFFSET >= MAX_CONTEXT_RECORDS);
        assert_eq!(RETRIEVAL_RANKING_VERSION, 3);
        assert!((0.0..=0.25).contains(&RETRIEVAL_TEMPORAL_BONUS_FRACTION));
        assert!(RETRIEVAL_RECENCY_HALF_LIFE_DAYS > 0.0);
        assert!(RETRIEVAL_SESSION_CAP > 0);
        assert!((0.0..=1.0).contains(&RETRIEVAL_RECENCY_SHARE));
    }
}
