# Retrieval behavior and limits

## Matching saved metadata

Recall searches permitted titles, observed search terms, and explicit corrections. It does not read page bodies or browser profile files. Returned search records include the saved title when it differs from the search terms, so values used to support a result remain visible.

Latin lexical matching uses SQLite's `unicode61` tokenizer for both discovery and admission. Accents and equivalent combining forms follow the database index's normalization. Words must match whole tokens: `rust` does not match `trust`. Short terms such as `uv`, `M3`, and `X1` remain searchable; common function words are excluded. A request contributes at most 32 distinct lexical terms.

Japanese and Chinese phrases of at least two characters have a separate, bounded discovery path over the same permitted metadata. It matches literal character sequences, including a phrase inside a longer unsegmented title. It does not provide general word segmentation or translation. The existing 10,000-atom scan bound, candidate limits, privacy filters, and request deadline still apply; it creates no extra index or background work.

## Values and questions

Exact dates, versions, unit-bearing quantities, supported currency amounts, and model identifiers remain distinct constraints. Currency amounts retain their complete value, including cents. Unambiguous currency codes and symbols agree (`GBP` and `£`, for example); `$` alone does not identify USD, and `¥` alone does not identify JPY or CNY. Ambiguous numeric punctuation is compared literally rather than assigned a locale.

Identifier normalization removes one hyphen joining an alphabetic prefix to a digit: `RX100` and `RX-100` agree, but `USB-C` and `USB-A` do not. This is a spelling rule, not a product alias database; identifiers whose only meaningful difference is that joining hyphen cannot be distinguished. Bare numbers without a recognized unit are ordinary search terms.

Recognized quantity questions such as “How much RAM does the laptop have?” require evidence in the corresponding dimension: a weight cannot answer a RAM question. Supported families are data, mass, length, time, power, voltage, and frequency. Unit symbols retain case where it changes meaning (`GB` versus `Gb`, `MHz` versus `mHz`); unrecognized case variants fail closed. No unit conversion is performed. Ambiguous questions about “size,” “capacity,” or “how much” keep ordinary research behavior.

An explicit leading quantity target controls the answer dimension: “How much time did I spend?” requires a duration, while “How much did I spend on RAM?” remains a money question. Both require current confirmation of the personal action. Explicit currency amounts still impose the monetary contract.

Price questions require monetary evidence; a currencyless rate such as `0.30/kWh` does not qualify. Supported currencies are USD, EUR, GBP, JPY, and CNY, with the symbol ambiguity described above. Asking what the user selected, ordered, bought, paid, or spent requires an explicit current confirmation; seeing a product page does not establish that action. These are finite wording rules, not general language understanding. Unknown personal-decision wording can still need interpretation by the assistant, which must respect each record's `observed` or `confirmed` state.

Numeric ranges are not evaluated. Recognized forms such as `under €500`, `from €500 to €1000`, and adjacent bounds joined by a hyphen or en dash return no records and a warning, as do malformed unit-bearing or monetary values that cannot be interpreted safely. A money question ending in `in EUR` (or another uppercase three-letter code) also abstains unless the query supplies a supported exact amount in that currency. This is a finite grammar; it does not recognize every currency or range expression. Use a specific amount and unit when exact matching is required.

## Comparisons and facets

The explicit two-model form `Compare X1 16GB and X2 32GB laptops` gives each model its own leading numeric qualifier. The suffix `laptops` is shared. The existing form `Compare X1 and X2 16GB laptops` applies the trailing qualifier to both models. More complex comparisons may fall back to ordinary retrieval; each result retains its original metadata and provenance.

Facets retain the existing behavior: semantic facets can affect ordering, while exact facets can also affect admission and impose numeric or identifier requirements. They are therefore not uniformly advisory. A proposed main-query-only replacement remains deferred because it did not meet the existing multilingual development gate. Facets should not introduce facts or constraints absent from the user's question.

## Activity windows

The current activity key is `source:capture_epoch:floor(timestamp/1800)`, where timestamps are Unix seconds and each bucket spans 30 minutes. Counts of distinct keys are shown in the UI as activity windows. The wire field `sessions` remains unchanged for compatibility and carries this activity-window count.

Two observations only two seconds apart can fall on opposite sides of a bucket boundary and count as two windows. Increasing `capture_epoch` can also produce a new key for activity in the same time bucket. Do not read these counts as statistically or behaviorally independent observations.

> Activity counts use fixed 30-minute windows; they are not independent confirmations.

## Provisional topic groups

Topic cards group related observations as provisional activity summaries. They are not beliefs, confirmed intentions, or objective descriptions of a person. New observations can change the groups.

Assignment is greedy and depends on arrival order: each observation is compared with the topics and capped centroids available at that point. The characterization test in `crates/serein-core/tests/activity_windows.rs` uses synthetic unit vectors at 0°, 50°, and 100°. It records different group partitions for the orders `[0, 1, 2]` and `[1, 2, 0]`; this function-level example does not show that natural page titles produce those vectors.

## Compatibility and limits

This behavior keeps the stored activity keys, the legacy wire field `sessions`, and the existing scoring formulas and thresholds. The labels clarify what the counts represent; they do not fix bucket-boundary sensitivity. Treat the resulting counts and topic groups as coarse activity evidence, not independent corroboration.

Direction ranking recognizes a narrow `from X to Y` phrase with single-word endpoints, including a leading “How to” infinitive. It changes the order of evidence already admitted by recall. Ambiguous names, multiple directions, and most trailing phrases keep ordinary ranking. A negative statement such as “migration from Linux to Windows is not supported” can still be relevant to that direction. The static encoder still loses general word order, including some negation and actor-role distinctions; this ranking rule does not resolve those limitations.
