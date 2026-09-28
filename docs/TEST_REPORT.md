# Serein v0.1.3 test report

The latest package target is v0.1.3 for macOS Apple silicon. The v0.1.2 section below preserves the previous release record; the final section records the v0.1.3 dashboard and retrieval update. Browser-path evidence is separate from labeled synthetic retrieval evaluation.

## v0.1.3 gate snapshot · 2026-09-28

| Gate | Status |
| --- | --- |
| Rust release-mode workspace tests | Passed on the final helper: 39 core unit, calibration, dashboard, evaluation, journey, model-parity, and 15 privacy tests. |
| Extension TypeScript, 42 Vitest tests, Chrome and Firefox production builds | Passed. |
| Package scanner | Passed for four unsigned artifacts; the final documentation refresh was rescanned before publication. |
| Chrome end-to-end | Passed against the final v0.1.3 helper in isolated Chrome for Testing 148.0.7778.96. Pairing, native ingest, duplicate ACK, queue/alarm, relink, recall/forget, dashboard, popup, keyboard focus, locales, and permissions were exercised. |
| Firefox end-to-end | Passed against the final v0.1.3 helper in Firefox 156.0.1 with temporary extension install, native pairing, ingest and duplicate ACK, exclude-and-forget, persistent pause, and system dark-theme default. The harness created only a guarded temporary user-level native-host manifest and removed it after the run. |
| Held-out retrieval and dashboard evaluation | The fresh v2 retrieval holdout was scored once after the final retrieval rule was frozen. The dashboard holdout was scored after its display rule was frozen. Synthetic-only scores are recorded below. |

## v0.1.2 release checks

| Area | Result and scope |
| --- | --- |
| Rust tests | `cargo test --release --workspace --offline` passed: 30 core unit tests, the evaluation and journey suites, two model-parity tests, and 15 privacy tests. Coverage includes transactional ingestion, retries, stale epochs, deletion lineage, model migration and deterministic topic rebuild, concurrent callers, exact identifiers/units, the 10,000-atom ceiling, bounded recall/explain and corrections. |
| Extension | WXT preparation, TypeScript `--noEmit`, 42 Vitest tests, and Chrome and Firefox MV3 production builds passed. The catalog and placeholder checks cover all six UI languages. |
| Native, install and wire harnesses | Native bridge, skill bundle, install/uninstall, and wire-contract checks passed. This includes 14 valid contract examples and 13 unknown-field mutation checks. The 800-atom process recall benchmark passed. |
| Google Chrome for Testing 148.0.7778.96 | The v0.1.2 extension loaded with native messaging. Automatic pairing through the bundled skill, queued privacy controls, ingest, duplicate ACK, skill reader recall and exclude-and-forget passed. The first-run prompt appeared with an empty paired vault and disappeared after the first saved observation. Batching checks covered 19 events without a helper launch, 20 events with one launch, an aged event, and a real browser alarm with the extension page closed. Relinking preserved the connection and vault. Consent defaults, persistent pause, keyboard focus, icon paths, minimal permissions, six-language persistence and screenshots were checked. |
| Firefox 156.0.1 | v0.1.2 temporary unsigned extension and native-messaging E2E passed: pairing, ingest, duplicate ACK, exclude-and-forget, persistent pause and system dark-theme default. Permanent installation signing remains untested. |
| System theme behavior | The v0.1.2 Chrome check covered OS dark/light switching and a saved manual override; the Firefox check covered the simulated dark OS preference. The default follows the system preference; a manual choice persists. |
| Native process harness | Framing, setup, pairing, ingestion, retry, deletion and bounded process exit passed. The process harness and real-browser runs used the installed model pack. |
| Skill installer integration | In the earlier public-repository check, the built-in Codex GitHub installer downloaded the skill and its installed payload passed setup. The v0.1.2 relocated skill connects with the bundled helper after executable bits are removed, exercising archive-installer behavior and paths containing spaces. Setup preserves an existing GitHub-installed skill and user-edited registration; owned registration upgrades retain the vault. Discovery and triggered execution inside all six assistants remain unverified. |
| Model conversion | Pinned upstream, 256 dimensions, row-wise int8. 264 conversion fixtures: exact token IDs, minimum cosine 0.99996468; 18 non-tied ranking comparisons preserved. Rust parity covers 12 texts across six languages. This measures conversion fidelity, not usefulness. |

## v0.1.2 real-site research demo

The demo at [`tests/research-demo.mjs`](../tests/research-demo.mjs) ran with `node tests/research-demo.mjs`, using a fresh isolated Google Chrome for Testing 148.0.7778.96 profile and a synthetic research identity. After the extension's consent default was checked and test consent enabled, it navigated public pages about office-chair and workstation ergonomics. Serein captured five actual tab-metadata observations through its normal Chrome tabs observer, delivered the real extension outbox through native messaging, and showed five cards in the dashboard. No source-page body or DOM text was read, no account was used, and no observation fixture was injected. The browser batch alarm was accelerated to 750 ms for this run; the normal extension alarm and native delivery path handled the captured records.

| Public page | Captured metadata |
| --- | --- |
| [Amazon search for ergonomic office chairs](https://www.amazon.com/s?k=ergonomic+office+chair) (`www.amazon.com`) | Search term: “ergonomic office chair” |
| [UCLA workstation setup guide](https://ergonomics.ucla.edu/office-ergonomics/4-steps-set-your-workstation) (`ergonomics.ucla.edu`) | “4 Steps to Set Up Your Workstation \| Ergonomics” |
| [CCOHS ergonomic-chair guide](https://www.ccohs.ca/oshanswers/ergonomics/office/chair.html) (`www.ccohs.ca`) | “CCOHS: Office Ergonomics - Ergonomic Chair” |
| [HSE DSE workstation checklist](https://www.hse.gov.uk/pubns/ck1.htm) (`www.hse.gov.uk`) | “Display screen equipment (DSE) workstation checklist - HSE” |
| [Cornell workstation guidelines](https://ergo.human.cornell.edu/ergoguide.html) (`ergo.human.cornell.edu`) | “CUergo: Computer Workstation Ergonomics Guidelines” |

The five retained pages returned HTTP 200. The [OSHA workstation page](https://www.osha.gov/computer-workstations) returned HTTP 403 and was not retained. [Reddit's office-chair discussion](https://www.reddit.com/r/OfficeChairs/comments/1oasp4s/) returned HTTP 200 with a challenge page and was not retained. These attempts produced no Serein cards. The deterministic order/dwell seed is represented in the demo by SHA-256 fingerprint `a0bc9607ddce3923`; the source seed is not stored. The [light screenshot](screenshots/research-demo-light.png) and [dark screenshot](screenshots/research-demo-dark.png) show the five actual cards and semantic-index availability in the **v0.1.2** dashboard. They are historical capture evidence, not screenshots of the v0.1.3 research-group layout.

The original capture-only run passed and produced the five-card screenshots above. A later attempt to repeat the run with a fresh profile and add a skill-reader check could not reproduce capture on this host: the headed Chrome window reported `focused=false`, and the Mac was locked. The repeat stopped before a `skills/serein-context/scripts/recall.sh` query could run. Reproducibility from a fresh profile and skill recall over this captured vault therefore remain unverified; the demo script keeps its foreground, queue, and dashboard assertions enabled.

## Constructed research journey evaluation

The new ranking check uses 84 page-title and hostname records across five constructed research journeys, with 54 hand-curated queries. The journeys cover a camera comparison, Python tooling, reduced-motion web design, Spanish-language coffee research, and Japanese balcony gardening. Eight queries expect no match. The titles and hostnames came from public web searches; relative dates and repeat sessions were invented to exercise ranking. This harness feeds constructed metadata directly into an isolated vault. It does not record browser activity or validate actual user history.

The v0.1.2 ranker added a recency channel to reciprocal-rank fusion over the existing candidate pool, with session count used as a tie-break. Compared with the preceding ranking, those historical results were mixed:

| Evaluation slice | Previous ranking | Recency and session signal |
| --- | --- | --- |
| Development journeys a–c, repeated runs: macro Recall@6 | 0.7704 | 0.7815 |
| Held-out Spanish journey d: Recall@6 | 0.935 | 0.907 |
| Held-out Spanish journey d: nDCG@6 | 0.777 | 0.800 |
| Held-out Japanese journey e: Recall@6 / nDCG@6 | 0.667 / 0.677 | 0.667 / 0.677 |
| Expected-empty queries returned empty | 3 of 8 | 3 of 8 |

This is not evidence of improved overall or field accuracy: the development slice gains slightly, Spanish recall falls as ordering improves, Japanese results do not change, and abstention remains weak. The labels are illustrative and hand-curated, not human-validated. Machine-readable [baseline](journey-evaluation-baseline.json) and [current results](journey-evaluation-results.json) preserve the per-query outputs and invariant checks.

## v0.1.2 retrieval fixture suite

The 90 fixtures are illustrative synthetic examples across EN, DE, NL, FR, JA and ZH. Six redacted sensitive-topic placeholders are unsupported for classifier evaluation; 84 cases are evaluated. The private-session fixtures exercise a native paused-policy gate, not browser incognito capture end to end.

With the real pack, 55 cases exactly matched, 18 correctly returned empty, 10 missed expected evidence and one returned a partial or extra result. There were 67 relevant returned observations out of 78 expected observations; 20 semantic-paraphrase cases returned relevant evidence, and four expected observations appeared only in the semantic run. Lexical fallback had 26 exact matches, 32 correct empty results, 10 misses and 16 unexpected results under its separately specified labels. Neither mode returned results on the explicit no-match cases. No byte-bound, duplicate-ID, attribution or paused-policy invariant failures were recorded. These counts do not establish field accuracy or sensitive-classifier coverage. Real-world retrieval accuracy has not been established.

## Performance

The v0.1.2 release build indexed all 10,000 synthetic atoms. Across 30 warm recall calls including process launch, indexed hybrid recall measured median 242.26 ms and p95 247.34 ms; every call completed inside the 1,500 ms budget. The [benchmark record](benchmark-10k.md) includes the method, lexical comparison and refresh timing. The separate 800-atom process harness measured indexed recall at median 118.81 ms / p95 120.40 ms across 25 warm calls. Cold-cache performance and the specified dual-core/4 GiB reference machine were not measured.

## v0.1.2 limits

Full topic merge/split editing, coverage/relation selection, calibrated multilingual retrieval and precise inactivity-based session grouping are outside this release or require further validation. The paired-answer study, real skill-triggered execution in all six assistant clients, clean-machine matrix, macOS signing and permanent Firefox signing have not been completed. This release bundles only macOS Apple silicon executables. Chrome was tested using Google Chrome for Testing; managed browser policies and other Chromium browsers were not tested. Most browser harnesses inject synthetic observations into the queue or native bridge to verify delivery and storage; the real-site demo above separately verifies metadata capture on the listed public pages. The Firefox harness does not measure the batch timer or helper invocation count. Assistant command approvals depend on each local harness; cloud and sandboxed executors cannot use a vault they cannot access. The UI provides observed context and explicit corrections; it does not present unvalidated inferred interests as confirmed facts.

Machine-readable evidence is adjacent to this report: browser-test-results.json, firefox-test-results.json, native-test-results.json, install-test-results.json, skill-installer-test-results.json, model-pack-report.json, evaluation-results.json and performance-results.json. Actual UI captures are in screenshots/. The release workflow records artifact checksums and signing status.

## v0.1.3 dashboard and retrieval update

The dashboard now presents model-supported research groups first, selected standalone observations next, and the bounded raw activity list in a collapsed disclosure. Group labels and prominence are heuristics over saved evidence; they are not confirmed preferences or psychological conclusions. Corrected observations remain inspectable in raw activity but are excluded from research groups and prominence.

Recall keeps bounded lexical, semantic, and confirmed-feedback candidate channels. It admits evidence based on direct lexical coverage, strong semantic agreement, or a specific exact facet in the saved text. Temporal activity contributes a capped ordering bonus and cannot replace a clear relevance match. These are deterministic retrieval rules, not learned confidence estimates.

On the existing six-language development suite, the v0.1.3 rule returned **60/78** expected observations and relevant evidence for **15/30** semantic-paraphrase cases, with no result for the explicit no-match cases. The earlier release returned 67/78 and 20/30 on this same illustrative suite. The stricter gate trades some recall for narrower admission; the separate held-out result below is the final validation checkpoint.

### Constructed v0.1.3 dashboard preview

The [light](screenshots/research-preview-light.png) and [dark](screenshots/research-preview-dark.png) previews show the actual v0.1.3 dashboard rendered in an isolated Chrome for Testing profile. The records are **constructed**: ten invented visits over two invented sessions, using five public-site title and hostname records that were observed in the earlier live v0.1.2 demo. The preview script sends those records through the native host's normal ingest contract, then verifies five raw cards, one research group, and an available local model before taking screenshots. The browser's tabs observer was not used to capture these invented visits. No user history, account, page body, or private profile was involved. The reproducible script is [`tests/research-preview.mjs`](../tests/research-preview.mjs), and the [result record](research-preview-results.json) states the capture mode and route.

### Evaluation method

Retrieval development and held-out evaluations use constructed page-title, hostname, and search metadata with hand-curated query-to-evidence labels. Relative times, dwell values, repeated sessions, and expected matches are synthetic inputs. The fixtures are loaded into isolated temporary vaults; they do not contain consented user browsing histories or user-labeled relevance judgments. Dashboard labels are also constructed and evaluate grouping and prominence behavior, not actual user preferences.

The browser-path demonstration is a separate method. Each synthetic research scenario uses a unique deterministic seed, a fresh isolated Chrome profile, and its own temporary vault. Only metadata exposed by Chrome's normal tabs observer and delivered through the real extension outbox and native helper counts as captured. This checks the capture and presentation path; it does not contribute retrieval labels or accuracy metrics. The checked-in public-page demo records one reproducible seed and one isolated profile. The three additional seeded journey definitions use distinct SHA-256 fingerprints: A specifies five public-site navigation attempts, while its four generated search phrases are marked as not browsed; B has two reached search URLs and four source-navigation failures; C has four search observations and six site-sequence entries with statuses. These are browser-metadata journey inputs, not relevance labels. No page body or DOM text is read, and failed or challenge-page navigations do not produce synthetic observations.

### Development ablation

On constructed development cases, lexical fallback was compared with the bundled model. The result is useful for tuning these fixtures; it is not held-out or real-user evidence.

| Development slice | Lexical fallback Recall@6 | Bundled model Recall@6 | Expected-empty specificity |
| --- | ---: | ---: | ---: |
| English/noise | 0.5087 | 0.6087 | 1.0 for both |
| Multilingual | 0.3819 | 0.7701 | 1.0 for both |

### Held-out results

The v2 retrieval holdout was generated independently with a discarded random seed and scored once after the final retrieval rules were frozen. It contains 40 constructed evidence records and 35 hand-curated questions. A seeded Chrome session verified metadata for five public pages; four sampled pages returned errors and one returned a challenge. Unverified fixture labels are marked as such and are not evidence that those pages were visited. The dashboard holdout was scored after its display rules were frozen. Both sets are constructed fixtures.

| Evaluation | Constructed set | Result |
| --- | --- | --- |
| Retrieval v2 | 35 questions, 23 with expected evidence | Macro Recall@6: **0.6522**. All **12/12** expected-empty questions returned no context. Some non-empty questions missed relevant evidence or returned extra items. |
| Dashboard grouping/prominence | 42 constructed observations | TP **24**, FP **1**, TN **12**, FN **5**; precision **0.9600**, recall **0.8276**, specificity **0.9231**. |

Aggregate development and held-out retrieval/dashboard results and seeded-scenario provenance are recorded in [calibration-results.json](calibration-results.json). The earlier relevance holdout scored 0.6722 Recall@6 and 9/11 expected-empty specificity before the final retrieval change; it remains a historical checkpoint, not final validation. The artifact contains aggregate evidence and seed fingerprints, not raw seeds or machine-specific paths.

These scores are not estimates of real-user accuracy, sensitivity, or confidence. Missed retrievals, extra results on some non-empty questions, and dashboard misses remain visible limitations; no field study or consented user-label study has been completed.

### Integration and browser status

The six required assistant targets are Claude Code, Codex, OpenCode, Hermes, OpenClaw, and a generic local executor. Installation/setup evidence and execution inside an assistant are separate:

| Target | Installation or setup evidence | Executed inside the assistant client |
| --- | --- | --- |
| Codex | The built-in GitHub skill installer downloaded the package in an earlier check; its installed payload passed setup. The v0.1.2 relocated package also passed the bundled-helper setup check. | No. Discovery and triggered recall remain unverified. |
| Claude Code, OpenCode, Hermes, OpenClaw | The shared skill package is available; no per-client installation result is recorded here. | No. Discovery and triggered recall remain unverified. |
| Generic local executor | Used by native and browser harnesses for setup and bounded calls. | Not an assistant-client run. |

A generic local harness does not count as execution inside any assistant client.

The v0.1.2 Firefox persistent-profile pairing issue remains unresolved. A restart or fresh-ticket retry appeared to restore the connected state in one local attempt, but that anecdote has not been reproduced as a verified fix. The exact cause remains unknown.

**v0.1.3 gate status:** Rust release-mode workspace tests, contract validation, extension TypeScript/Vitest, both browser production builds, native/skill/install harnesses, the four-artifact publication scan, and isolated Chrome and Firefox end-to-end passed on the final v0.1.3 helper. The fresh v2 retrieval holdout and dashboard holdout are recorded above. The current-layout preview is the labeled constructed replay described above. A current-layout live-site screenshot could not be captured on this locked Mac because the automated Chrome window did not receive foreground focus; the earlier live public-page screenshots remain labeled as v0.1.2 capture evidence. Real-user retrieval accuracy and triggered execution inside the named assistants remain unverified.
