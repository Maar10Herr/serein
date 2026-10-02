# Retrieval activity counts and topic groups

## Activity windows

The current activity key is `source:capture_epoch:floor(timestamp/1800)`, where timestamps are Unix seconds and each bucket spans 30 minutes. Counts of distinct keys are shown in the UI as activity windows. The wire field `sessions` remains unchanged for compatibility and carries this activity-window count.

Two observations only two seconds apart can fall on opposite sides of a bucket boundary and count as two windows. Increasing `capture_epoch` can also produce a new key for activity in the same time bucket. Do not read these counts as statistically or behaviorally independent observations.

> Activity counts use fixed 30-minute windows; they are not independent confirmations.

## Provisional topic groups

Topic cards group related observations as provisional activity summaries. They are not beliefs, confirmed intentions, or objective descriptions of a person. New observations can change the groups.

Assignment is greedy and depends on arrival order: each observation is compared with the topics and capped centroids available at that point. The characterization test in `crates/serein-core/tests/activity_windows.rs` uses synthetic unit vectors at 0°, 50°, and 100°. It records different group partitions for the orders `[0, 1, 2]` and `[1, 2, 0]`; this function-level example does not show that natural page titles produce those vectors.

## Compatibility and limits

This behavior keeps the stored activity keys, the legacy wire field `sessions`, and the existing scoring formulas and thresholds. The labels clarify what the counts represent; they do not fix bucket-boundary sensitivity. Treat the resulting counts and topic groups as coarse activity evidence, not independent corroboration.

Direction ranking recognizes a narrow `from X to Y` phrase with single-word endpoints. It changes the order of evidence already admitted by recall. Ambiguous names, multiple directions, and most trailing phrases keep ordinary ranking. A negative statement such as “migration from Linux to Windows is not supported” can still be relevant to that direction. The static encoder still loses general word order, including some negation and actor-role distinctions; this ranking rule does not resolve those limitations.
