# Serein 0.1.0 test report

This report covers the macOS Apple silicon release and records the tested scope and known limits. Browser tests use synthetic evidence and isolated profiles.

## Executed checks

| Area | Result and scope |
| --- | --- |
| Rust release tests | 22 passed: 15 privacy/storage tests, 4 installer tests, 2 model tests and 1 evaluation harness. Covers transactional ingestion, retries, stale epochs, deletion lineage, concurrent callers, exact identifiers/units, the 10,000 atom ceiling, bounded recall/explain and corrections. |
| Extension | 42 tests passed, including six-language catalog and placeholder checks; TypeScript check passed; Chrome and Firefox MV3 production builds passed. |
| Wire contracts | 14 valid examples and 13 unknown-field mutation checks passed. Schemas and Rust types are maintained separately. |
| Google Chrome for Testing 154.0.8037.57 | Actual loaded extension and native messaging: automatic pairing through the bundled skill, queued privacy controls, ingest, duplicate ACK, skill reader recall and exclude-and-forget. Batching checks cover 19 events without a helper launch, 20 events with one launch, an aged event, and a real browser alarm with the extension page closed. Relinking preserves the existing connection and vault. Consent defaults, persistent pause, keyboard focus, icon paths, minimal permissions, six-language persistence and screenshots checked. |
| Firefox 156.0.1 | Actual temporary unsigned extension/native messaging: automatic pairing through the bundled skill, ingest, duplicate ACK, exclusion/deletion and persistent pause passed. Permanent installation signing remains untested. |
| Native process harness | Framing, setup, pairing, ingestion, retry, deletion and bounded process exit passed. The process harness and real-browser runs used the installed model pack. |
| Skill installer integration | The built-in Codex GitHub installer downloaded the complete skill from the public repository; its installed payload passed setup. A relocated skill connects with the bundled helper after executable bits are removed, exercising archive-installer behavior and paths containing spaces. Setup preserves an existing GitHub-installed skill and user-edited registration; owned registration upgrades retain the vault. Discovery and triggered execution inside all six assistants remain unverified. |
| Model conversion | Pinned upstream, 256 dimensions, row-wise int8. 264 conversion fixtures: exact token IDs, minimum cosine 0.99996468; 18 non-tied ranking comparisons preserved. Rust parity covers 12 texts across six languages. This measures conversion fidelity, not usefulness. |

## Retrieval evidence and limits

The 90 fixtures are illustrative synthetic examples across EN, DE, NL, FR, JA and ZH. Six redacted sensitive-topic placeholders are unsupported for classifier evaluation; 84 cases are evaluated. The private-session fixtures exercise a native paused-policy gate, not browser incognito capture end to end.

With the real pack, 55 cases exactly matched, 18 correctly returned empty, 10 missed expected evidence and one returned a partial or extra result. There were 67 relevant returned observations out of 78 expected observations; 20 semantic-paraphrase cases returned relevant evidence, and four expected observations appeared only in the semantic run. Lexical fallback had 26 exact matches, 32 correct empty results, 10 misses and 16 unexpected results under its separately specified labels. Neither mode returned results on the explicit no-match cases. No byte-bound, duplicate-ID, attribution or paused-policy invariant failures were recorded. These counts do not establish field accuracy or sensitive-classifier coverage. Real-world retrieval accuracy has not been established.

## Performance

At 800 synthetic atoms with 25 warm samples including process launch, ingest of 32 events measured median 83.16 ms / p95 86.48 ms; indexed recall measured median 248.57 ms / p95 278.43 ms. The maximum RSS across sequential subprocesses launched by the harness was 106,725,376 bytes (about 102 MiB); this may exceed the recall process peak. Cold-cache performance, 10,000-atom performance and the specified dual-core/4 GiB reference machine were not measured. The 10,000-atom admission ceiling is separately tested.

## Limits

Full topic merge/split editing, coverage/relation selection, calibrated multilingual retrieval and precise inactivity-based session grouping are outside this release or require further validation. The paired-answer study, real skill-triggered execution in all six assistant clients, clean-machine matrix, macOS signing and permanent Firefox signing have not been completed. This release bundles only macOS Apple silicon executables. Chrome was tested using Google Chrome for Testing; managed browser policies and other Chromium browsers were not tested. Browser harnesses inject synthetic observations into the queue or native bridge: they verify delivery and storage, not every website’s metadata capture. The Firefox harness does not measure the batch timer or helper invocation count. Assistant command approvals depend on each local harness; cloud and sandboxed executors cannot use a vault they cannot access. The UI provides observed context and explicit corrections; it does not present unvalidated inferred interests as confirmed facts.

Machine-readable evidence is adjacent to this report: browser-test-results.json, firefox-test-results.json, native-test-results.json, install-test-results.json, skill-installer-test-results.json, model-pack-report.json, evaluation-results.json and performance-results.json. Actual UI captures are in screenshots/. The release workflow records artifact checksums and signing status.
