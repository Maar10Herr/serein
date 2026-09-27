//! Canonical, build-time algorithm parameters.
//! These are calibration values, not probabilities or user preferences.
pub const DIMENSIONS: usize = 256;
pub const QUERY_WEIGHT: f32 = 0.7;
pub const TITLE_WEIGHT: f32 = 0.3;
pub const TOPIC_ADMISSION: f32 = 0.62;
pub const TOPIC_MARGIN: f32 = 0.08;
pub const RUNNER_UP_ADMISSION: f32 = 0.55;
pub const SEMANTIC_RECALL_ADMISSION: f32 = 0.55;
pub const MAX_TOPICS: usize = 128;
pub const MAX_CONTEXT_RECORDS: usize = 6;
pub const MAX_RECORDS_PER_SITE: u32 = 2;
pub const ACTIVITY_HALF_LIVES_DAYS: [f64; 3] = [1.0, 7.0, 30.0];
pub const ACTIVITY_PRIOR_STRENGTH: f64 = 2.0;
pub const BURST_DAILY_RATE_PRIOR: f64 = 0.01;
pub const RRF_OFFSET: usize = 60;
pub const RETRIEVAL_CANDIDATES_PER_CHANNEL: usize = 64;
pub const CONFIRMED_FEEDBACK_CANDIDATES: usize = 32;

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
        assert!(MAX_TOPICS >= MAX_CONTEXT_RECORDS);
        assert!(MAX_RECORDS_PER_SITE as usize <= MAX_CONTEXT_RECORDS);
        assert!(ACTIVITY_HALF_LIVES_DAYS
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(ACTIVITY_PRIOR_STRENGTH > 0.0 && BURST_DAILY_RATE_PRIOR > 0.0);
        assert!(RRF_OFFSET >= MAX_CONTEXT_RECORDS);
    }
}
