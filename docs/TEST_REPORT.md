# Serein beta test report

This report covers the macOS arm64 beta build and separates measured checks from unverified release gates. Browser tests use synthetic evidence and isolated profiles.

## Executed checks

| Area | Result and scope |
| --- | --- |
| Rust release tests | 21 passed: 15 privacy/storage tests, 3 installer tests, 2 model tests and 1 evaluation harness. Covers transactional ingestion, retries, stale epochs, deletion lineage, concurrent callers, exact identifiers/units, the 10,000 atom ceiling, bounded recall/explain and corrections. |
| Extension | 42 tests passed, including six-language catalog and placeholder checks; TypeScript check passed; Chromium and Firefox MV3 production builds passed. |
| Wire contracts | 14 valid examples and 13 unknown-field mutation checks passed. Schemas and Rust types are maintained separately. |
| Chromium 148.0.7778.96 | Actual loaded extension and native messaging: pairing, queued privacy controls, ingest, duplicate ACK, CLI recall and exclude-and-forget. Consent defaults, persistent pause, keyboard focus, icon paths, minimal permissions, six-language persistence and screenshots checked. |
| Firefox 156.0.1 | Actual temporary unsigned extension/native messaging: pairing, ingest, duplicate ACK, exclusion/deletion and persistent pause passed. Permanent installation signing remains untested. |
| Native process harness | Framing, setup, pairing, ingestion, retry, deletion and bounded process exit passed. The recorded process-only run used lexical fallback; the real-browser runs above exercised the installed model pack. |
| Skill installer integration | Setup carries a validated repository URL and explicit install choice. Fake-launcher tests cover command selection, arguments and failure handling; isolated setup preserves an existing GitHub-installed `SKILL.md` and user edits. The real Skills CLI was not executed because automatic review rejected the external installer invocation. Skill discovery and triggered execution inside all six assistants remain unverified. |
| Model conversion | Pinned upstream, 256 dimensions, row-wise int8. 264 conversion fixtures: exact token IDs, minimum cosine 0.99996468; 18 non-tied ranking comparisons preserved. Rust parity covers 12 texts across six languages. This measures conversion fidelity, not usefulness. |

## Retrieval evidence and limits

The 90 fixtures are illustrative synthetic examples across EN, DE, NL, FR, JA and ZH. Six redacted sensitive-topic placeholders are unsupported for classifier evaluation; 84 cases are evaluated. The private-session fixtures exercise a native paused-policy gate, not browser incognito capture end to end.

With the real pack, 55 cases exactly matched, 18 correctly returned empty, 10 missed expected evidence and one returned a partial or extra result. There were 67 relevant returned observations out of 78 expected observations; 20 semantic-paraphrase cases returned relevant evidence, and four expected observations appeared only in the semantic run. Lexical fallback had 26 exact matches, 32 correct empty results, 10 misses and 16 unexpected results under its separately specified labels. Neither mode returned results on the explicit no-match cases. No byte-bound, duplicate-ID, attribution or paused-policy invariant failures were recorded. These counts do not establish field accuracy or sensitive-classifier coverage. Semantic quality acceptance remains open.

## Performance

At 800 synthetic atoms with 25 warm samples including process launch, ingest of 32 events measured median 83.16 ms / p95 86.48 ms; indexed recall measured median 248.57 ms / p95 278.43 ms. The maximum RSS across sequential subprocesses launched by the harness was 106,725,376 bytes (about 102 MiB); this may exceed the recall process peak. Cold-cache performance, 10,000-atom performance and the specified dual-core/4 GiB reference machine were not measured. The 10,000-atom admission ceiling is separately tested.

## Open release gates

Full topic merge/split editing, coverage/relation selection, calibrated multilingual retrieval and precise inactivity-based session grouping require further implementation or validation. The paired-answer study, real skill-triggered execution in all six assistant clients, clean-machine cross-platform matrix, macOS signing and permanent Firefox signing remain outstanding. The UI provides observed context and explicit corrections; it does not present unvalidated inferred interests as confirmed facts.

Machine-readable evidence is adjacent to this report: browser-test-results.json, firefox-test-results.json, native-test-results.json, install-test-results.json, model-pack-report.json, evaluation-results.json and performance-results.json. Actual UI captures are in screenshots/. The release workflow records artifact checksums and signing status.
