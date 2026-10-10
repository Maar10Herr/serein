# Retrieval study · October 2026

This document preserves the October 2026 retrieval study that informed v0.2.1. It compares frozen quality, optimized, and duration-corrected source snapshots with released v0.2.0; each result below remains attributed to its recorded snapshot. The bounded lexical and query-contract changes studied here were incorporated into v0.2.1. See the [v0.2.1 test report](../../TEST_REPORT.md) for final package, browser, and CI evidence.

## What changed

- Discovery and admission use SQLite's own `unicode61` normalization. Latin terms match whole tokens; accents, short names and model identifiers retain support.
- A bounded Japanese/Chinese phrase scan recovers literal sequences of at least two characters inside unsegmented metadata. It uses the existing source/recency index, privacy filters, deadline and candidate limits, with no new persistent index.
- Query contracts distinguish money from quantities and require a compatible quantity dimension. Exact amounts retain cents, signs and currency identity. Byte/bit and case-sensitive SI units remain distinct. Recognized unsupported ranges return an explanatory warning instead of becoming equality constraints.
- The narrow two-model comparison grammar assigns leading numeric qualifiers to their respective models. Explicit personal selection, order and spending questions require current confirmed evidence.
- Search results include the saved page title when it supplied evidence absent from the search terms. The direction rule accepts a leading “How to” infinitive.

The model, fusion weights, thresholds, candidate ceilings, schema, wire contracts, browser permissions and integration adapters were unchanged by these retrieval changes. [Retrieval behavior](../../RETRIEVAL.md) documents the finite grammar and remaining limits.

## Hypotheses that did not justify a change

The proposed latest-1,000 confirmation displacement did not reproduce in a supported vault: the public API permits at most 1,000 confirmation rows globally. A regression covers the boundary, a latest replacement, suppression and rejection of row 1,001. Widening the scan would not fix the reported case.

Main-query-only facet admission remains deferred. The isolated T03 retry scored 0.5243 on the multilingual development fixture against an unchanged 0.60 floor; an optimistic support ceiling was only 0.5382. Exact facets retain their existing admission behavior and are documented accordingly. Suppressed records remain suppressed.

No ranking replacement was selected. The quality candidate's native stage trace contained 32 relevant query/document judgments: 24 reached the final packet, seven failed support admission, and one failed an exact constraint in a negated query. None was lost solely during final ordering or packet selection. Equal-RRF and diversity variants were not evaluated as production candidates; component results from the handoff are not native performance claims.

## Evaluation method

The baseline was released v0.2.0, commit `6ab4722501813b71549627a04a847802f35f99fe`. At study time, the quality candidate was an uncommitted source patch over that commit, frozen under [source and executable hashes](candidate-identity.json) before the sealed evaluation. A subsequent performance-only change is identified in [the optimized candidate manifest](optimized-candidate-identity.json); the duration correction is identified in [the final source manifest](final-candidate-identity.json). No candidate commit or CI run existed at the study cutoff. This historical snapshot status does not describe the v0.2.1 release gate; see the [current test report](../../TEST_REPORT.md).

Only titles and hostnames enter disposable native vaults. URLs, page bodies, relevance labels and authored behavioral data are not retrieval inputs. Policy rejections are retained. The handoff corpus and the 158 earlier calibration queries were previously inspected regressions, not fresh holdouts. A separately authored synthetic batch was sealed before the quality-candidate freeze and evaluated only after that freeze.

Metrics retain authored labels and distinguish raw, attainable and payload-group recall. Expected-empty specificity uses only expected-empty queries; false-empty counts use positive queries. Small synthetic samples do not establish real-user accuracy. Timing includes process launch on macOS arm64; file caches were warm, and an unindexed first read is not an OS cold-cache measurement.

The paired runner's `group` fields preserve hostname plus title, and its `attainable` denominator accounts for policy, six slots and two records per site. Those fields do **not** model the runtime's additional cross-site payload collapse. [Supplemental native-payload scoring](native-payload-groups.json), calculated from the unchanged fresh packets and labels without rerunning queries, uses the actual whitespace-normalized title grouping for these single-source visit records. It reports both group recall and the distinct-group slot/site capacity ceiling; neither excuses a failed independent-source request.

## Handoff regression results

Of 49 public title/hostname records, 48 passed native ingestion. The existing sensitivity rule rejects the `py06` title because “archives” contains its `hiv` trigger. That policy false positive is retained, with no bypass. The corpus has 30 authored questions: 24 positives and six expected-empty cases.

| Metric | v0.2.0 | Quality candidate |
| --- | ---: | ---: |
| Macro Recall@6, 24 positive questions | 0.7639 | 0.8056 |
| Micro relevant query/document pairs returned | 23/32 | 24/32 |
| Macro nDCG@6 | 0.7564 | 0.7981 |
| Expected-empty questions returning no context | 6/6 | 6/6 |
| False-empty positive questions | 4/24 | 3/24 |

Only `pyq01` changes its returned document IDs: the candidate recovers the `uv` result. Raw, source-group and slot/site-attainable recall agree on this corpus. The remaining misses include cross-language heat-pump support, Japanese camera support, cooking queries and a negated model constraint. [All paired packets and slice metrics](handoff-pairs.json) and the [native stage trace](stage-loss.json) preserve the failures. The broader parser changes are supported by targeted native regressions, not by claiming that this corpus exercises every new rule.

The gain is in the six-question exact family (Recall@6 0.8333 → 1.0000). The constrained, multilingual and semantic families each contain six positives and stay at 0.8333, 0.7222 and 0.6667 respectively. Latin-script positives rise from 0.7667 to 0.8167 across 20 questions. The four non-Latin/mixed-script questions are unchanged; one Japanese camera query still returns nothing. These one-question script slices cannot establish language-level quality.

## Fresh synthetic results

The first sealed evaluation used 34 invented metadata records, 15 questions and 510 judgments. All 34 records passed native ingestion. Its primary acceptance set was authored as 14 questions: 12 positives and two expected-empty cases. One arm-local comparison was designated characterization-only before execution and stays separate. The fixture hash manifest was sealed as `28314fd205a1fc0f40666d5861ad5386a7134d6c3ea664c4a5abac3874429e82`. Replays after the quality-candidate freeze are seen regression checks, not additional fresh evaluation.

| Primary acceptance metric | v0.2.0 | Quality candidate |
| --- | ---: | ---: |
| Macro raw / source-identity-group Recall@6, 12 positives | 0.8750 | 0.8750 |
| Macro native-payload-group / slot/site-attainable group Recall@6 | 0.9167 | 0.9167 |
| Macro nDCG@6 | 0.8537 | 0.8537 |
| Expected-empty questions returning no context | 1/2 | 2/2 |
| False-empty positive questions | 0/12 | 1/12 |
| Labeled hard-negative query/document pairs returned | 10/17 | 6/17 |
| Packet size/count violations | 0 | 0 |

The new false empty asks which soldering station the user selected. The clean metadata-only pass contains no confirmation, so the stricter decision rule abstains. The unchanged positive label still counts this as a recall loss. In the separate feedback scenario, the candidate returns the latest selection text when `confirmed_preferences` is explicitly in scope, excludes the superseded text, and stays empty with research-only scope. Suppression succeeds in both scopes for both builds.

The characterized comparison now returns both correct models. Including it gives 15-question macro Recall@6 of 0.8077 → 0.8846 across 13 positives; this is supplemental, not a replacement for the sealed primary denominator. Its two additional hard-negative labels were not returned in either build.

The six remaining hard-negative returns are shared with the baseline: mixed-script connector context, a Chinese title match, a tank-capacity question, two travel-adapter distractors and a control-unit voltage question. A request for two independent source sites still retrieves only half its expected evidence. General attribute interpretation, source-aware duplicate handling and confirmed-text paraphrase retrieval remain open. Identifier spelling normalization also cannot distinguish IDs whose only meaningful difference is the joining hyphen documented in the retrieval contract.

After preserving these results, the capacity probe prompted one performance-only change: reject absent currency-qualifier syntax before invoking the answer-type parser. Grammar, labels, thresholds and fixtures remain unchanged. A subsequent code review found that “How much time did I spend?” was misclassified as money. A failing native confirmation test reproduced it; a leading quantity target now takes precedence over a later spending verb, with a money-on-RAM control. Replaying the original batch after either change is a seen regression check, not a second fresh evaluation. The first harness invocation stopped during schema validation before native setup or any query: the fixture used `provenance`, while the adapter expected `provenance_type`. The failed preflight was preserved; accepting that metadata-label alias allowed the single completed evaluation pass. The fixture and labels were unchanged. [Paired outputs and slice metrics](fresh-pairs.json) retain every query, hard-negative judgment, native packet and scoped feedback result.

The primary set contains 11 English questions (nine positive), one German, one Chinese and one mixed English/Japanese question. English positive Recall@6 stays at 0.8333; the three one-question language slices each remain at 1.0000. The quantity-versus-price family gains its relevant capacity record but retains a price distractor, while the clean metadata-only selection case loses its positive result as described above. All other primary-family recall values are unchanged. No language-specific threshold was fitted.

## Resource cost and performance follow-up

The 10,000-record capacity fixture uses invented Japanese titles and 20 permitted synthetic sites. One released native host ingests the seed, then each arm receives byte-identical pre-index snapshots. All 10,000 records are accepted and indexed in 40 refresh calls. The table below measures the optimized source before the final duration correction on macOS 15.7.3 arm64; warm latency includes a new process per sample (10 samples per query, sequential arms).

| Measurement | v0.2.0 | Optimized candidate |
| --- | ---: | ---: |
| Common two-character `東京`, median / p95 | 226.8 / 235.3 ms | 277.5 / 282.4 ms |
| Rare two-character `青磁`, median / p95 | 177.1 / 179.3 ms | 157.9 / 161.5 ms |
| Rare target returned in top six | No | Yes |
| First recall with an unbuilt index | 134.4 ms | 310.5 ms |
| Full index build, 40 calls | 5.646 s | 5.664 s |
| Peak measured process RSS | 83.87 MB | 83.77 MB |
| Database after indexing | 25.60 MB | 25.60 MB |
| Database growth from indexing | 16.95 MB | 16.95 MB |

Both builds return six matching titles for the common query. That is top-six token precision, not recall over 10,000 positives. Both first reads report partial results with 9,744 atoms still pending. Final database sizes match within each pair; page allocation differs slightly between separately created fixtures.

In this comparison, the optimized snapshot's common-query median was about 22% higher than v0.2.0. The first quality-candidate probe was slower still, at 409.8 ms versus 226.1 ms; moving the cheap currency-qualifier syntax check before full answer parsing reduced that cost in the subsequent run. These are separate samples, not an interleaved causal benchmark. [The original quality-candidate probe](capacity-quality-candidate.json) and [the optimized probe](capacity-optimized-candidate.json) retain all timings and hashes.

Seed ingestion took 17.712 seconds for 625 batches of 16 (about 1.771 ms per attempted event, including launches and the loop). This is one baseline-host write measurement, **not** an independently measured candidate write comparison. WAL and shared-memory sizes are measured after subprocess exit/checkpoint, not at peak transaction size. OS caches were not evicted; neither a true cold-cache test nor transient peak WAL growth was measured. The capacity fixture is a local synthetic probe, not browser or real-user validation. At the study cutoff, capacity timing had not been repeated on the duration-corrected executable; these measurements therefore remain attributed to the separately hashed optimized snapshot, not the exact v0.2.1 package. The later capacity check on the exact v0.2.1 package is recorded separately in the [current test report](../../TEST_REPORT.md).

## Final regression comparison

Recreated disposable vaults produced some different ordering: one synthetic quantity question swapped its relevant and hard-negative results; two handoff queries changed a non-relevant second result, and another swapped two results. Recall and the synthetic hard-negative counts stayed unchanged, but nDCG and MRR did not. The [separate-vault differences](separate-vault-replay-differences.json), [synthetic replay](seen-optimized-replay.json) and [handoff replay](handoff-optimized-replay.json) preserve these outcomes.

To isolate source changes from newly generated record IDs, a final replay used byte-identical, fully indexed database clones for the quality, optimized and duration-corrected builds. All 45 previously seen questions matched across all three binary pairs: document sets, ordering, text, evidence state, status, index fields, alternatives and warnings. UUIDs and timestamps were normalized; ranks and text were not. This [identical-state check](same-vault-parity.json) found no retrieval-output difference for these questions. It is a regression check, not fresh evidence of accuracy, and does not establish order stability across independently created vaults.

Both feedback sidecars also matched between the preserved quality run and the optimized replay after excluding process timing telemetry: suppression and latest-confirmation scope behavior were unchanged. The duration correction was covered separately by a reproduced unit/native failure and passing correction tests.

## Verification and scope

At study close, the duration-corrected source snapshot passed 182 workspace tests. The stage diagnostic was ignored by default and passed separately after the duration correction; its [final trace](stage-loss-final.json) retains the same 24 returned, seven support-admission and one exact-constraint outcome. A library run also verified original UTF-8 span offsets after normalizing a range separator. Formatting and a locked, offline release build passed. The four existing calibration fixtures passed their unchanged configured gates and packet invariants. These source-snapshot checks are separate from final v0.2.1 package validation, recorded in the [current test report](../../TEST_REPORT.md).

The earlier relevance fixture loses one expected match: a question asks how many minutes the user spent on a particular page, but the returned metadata contains no duration or personal confirmation. Its macro Recall@6 falls from 0.7347 to 0.6931; the label is retained. Other aggregate recall values and the four pre-existing expected-empty failures remain unchanged. [All 158 per-query comparisons](seen-regressions.json) retain gains, losses and unchanged outcomes.

The study itself added no Chrome, Firefox, installation or named-assistant execution result. Its six integration paths retain the installed-versus-tested distinctions described in the [current test report](../../TEST_REPORT.md); discovery and triggered recall inside the named assistant clients remain unverified. No live vault, browser profile or native registration was used for this study.

## Study outcomes and remaining limits

The bounded lexical, exact-value, comparison and confirmation fixes from this study were incorporated into v0.2.1. They address reproduced native failures without changing the model or tuning thresholds to these labels. The CJK discovery path adds useful literal matches at a measured workload cost; it should not be described as an overall speed improvement.

T03, a ranking replacement, general attribute interpretation and independent-source deduplication remain open. The confirmation-window hypothesis did not justify a wider scan, and the observed support losses did not justify changing RRF weights. The native [checks](checks.json) supported the study snapshots; they did not certify a release package or broader real-user accuracy. At study close, no candidate changes had yet been committed, pushed, or published; selected changes were later incorporated into v0.2.1. Current package, browser, and CI status is in the [test report](../../TEST_REPORT.md).

## Reproduction

```sh
cargo fmt --all -- --check
cargo test --workspace --release --locked --offline -- --test-threads=1
cargo build --workspace --release --locked --offline
```

Migration tests require the pinned prior-schema readers described in the [development guide](../../DEVELOPMENT.md). The optional stage diagnostic requires the hash-verified handoff `data` directory and a new output file under Git-ignored `.implementation`:

```sh
mkdir -p .implementation
SEREIN_RESEARCH_DATASET=/path/to/handoff/data \
SEREIN_RESEARCH_DIAGNOSTICS_OUTPUT="$PWD/.implementation/stage-loss.json" \
cargo test -p serein-core --lib --release --locked --offline \
  native_handoff_stage_loss_diagnostic -- --ignored --nocapture
```

The diagnostic creates and removes its own vault, bounds input sizes, verifies dataset hashes and refuses to overwrite an existing report. Its private-stage access is compiled only into tests; it adds no assistant-facing data endpoint.
