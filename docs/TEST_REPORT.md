# Serein 0.1.1 test report

This report covers the macOS Apple silicon release and records the tested scope and known limits. Browser checks use isolated profiles; the real-site demo below is clearly separated from tests that use synthetic evidence.

## Executed checks

| Area | Result and scope |
| --- | --- |
| Rust tests | Core, privacy, model-parity and optimized evaluation suites passed. Covers transactional ingestion, retries, stale epochs, deletion lineage, model migration and deterministic topic rebuild, concurrent callers, exact identifiers/units, the 10,000 atom ceiling, bounded recall/explain and corrections. |
| Extension | 42 tests passed, including six-language catalog and placeholder checks; TypeScript check passed; Chrome and Firefox MV3 production builds passed. |
| Wire contracts | 14 valid examples and 13 unknown-field mutation checks passed. Schemas and Rust types are maintained separately. |
| Google Chrome for Testing 148.0.7778.96 | Actual loaded extension and native messaging: automatic pairing through the bundled skill, queued privacy controls, ingest, duplicate ACK, skill reader recall and exclude-and-forget. Batching checks cover 19 events without a helper launch, 20 events with one launch, an aged event, and a real browser alarm with the extension page closed. Relinking preserves the existing connection and vault. Consent defaults, persistent pause, system theme, keyboard focus, icon paths, minimal permissions, six-language persistence and screenshots checked. |
| Firefox 156.0.1 | Actual temporary unsigned extension/native messaging: automatic pairing through the bundled skill, ingest, duplicate ACK, exclusion/deletion, persistent pause and system dark-theme default passed. Permanent installation signing remains untested. |
| System theme behavior | Passed in isolated Chrome via `tests/theme.mjs` (OS dark/light switch and saved manual override) and Firefox 156 via `tests/firefox_bridge.py` (simulated dark OS preference). The default follows the system preference; a manual choice persists. |
| Native process harness | Framing, setup, pairing, ingestion, retry, deletion and bounded process exit passed. The process harness and real-browser runs used the installed model pack. |
| Skill installer integration | The built-in Codex GitHub installer downloaded the complete skill from the public repository; its installed payload passed setup. A relocated skill connects with the bundled helper after executable bits are removed, exercising archive-installer behavior and paths containing spaces. Setup preserves an existing GitHub-installed skill and user-edited registration; owned registration upgrades retain the vault. Discovery and triggered execution inside all six assistants remain unverified. |
| Model conversion | Pinned upstream, 256 dimensions, row-wise int8. 264 conversion fixtures: exact token IDs, minimum cosine 0.99996468; 18 non-tied ranking comparisons preserved. Rust parity covers 12 texts across six languages. This measures conversion fidelity, not usefulness. |

## Real-site research demo

The demo at [`tests/research-demo.mjs`](../tests/research-demo.mjs) ran with `node tests/research-demo.mjs`, using a fresh isolated Google Chrome for Testing 148.0.7778.96 profile and a synthetic research identity. After the extension's consent default was checked and test consent enabled, it navigated public pages about office-chair and workstation ergonomics. Serein captured five actual tab-metadata observations through its normal Chrome tabs observer, delivered the real extension outbox through native messaging, and showed five cards in the dashboard. No source-page body or DOM text was read, no account was used, and no observation fixture was injected. The browser batch alarm was accelerated to 750 ms for this run; the normal extension alarm and native delivery path handled the captured records.

| Public page | Captured metadata |
| --- | --- |
| [Amazon search for ergonomic office chairs](https://www.amazon.com/s?k=ergonomic+office+chair) (`www.amazon.com`) | Search term: “ergonomic office chair” |
| [UCLA workstation setup guide](https://ergonomics.ucla.edu/office-ergonomics/4-steps-set-your-workstation) (`ergonomics.ucla.edu`) | “4 Steps to Set Up Your Workstation \| Ergonomics” |
| [CCOHS ergonomic-chair guide](https://www.ccohs.ca/oshanswers/ergonomics/office/chair.html) (`www.ccohs.ca`) | “CCOHS: Office Ergonomics - Ergonomic Chair” |
| [HSE DSE workstation checklist](https://www.hse.gov.uk/pubns/ck1.htm) (`www.hse.gov.uk`) | “Display screen equipment (DSE) workstation checklist - HSE” |
| [Cornell workstation guidelines](https://ergo.human.cornell.edu/ergoguide.html) (`ergo.human.cornell.edu`) | “CUergo: Computer Workstation Ergonomics Guidelines” |

The five retained pages returned HTTP 200. The [OSHA workstation page](https://www.osha.gov/computer-workstations) returned HTTP 403 and was not retained. [Reddit's office-chair discussion](https://www.reddit.com/r/OfficeChairs/comments/1oasp4s/) returned HTTP 200 with a challenge page and was not retained. These attempts produced no Serein cards. The deterministic order/dwell seed is represented in the demo by SHA-256 fingerprint `a0bc9607ddce3923`; the source seed is not stored. The [light screenshot](screenshots/research-demo-light.png) and [dark screenshot](screenshots/research-demo-dark.png) show the five actual cards and semantic-index availability.

The original capture-only run passed and produced the five-card screenshots above. A later attempt to repeat the run with a fresh profile and add a skill-reader check could not reproduce capture on this host: the headed Chrome window reported `focused=false`, and the Mac was locked. The repeat stopped before a `skills/serein-context/scripts/recall.sh` query could run. Reproducibility from a fresh profile and skill recall over this captured vault therefore remain unverified; the demo script keeps its foreground, queue, and dashboard assertions enabled.

## Retrieval evidence and limits

The 90 fixtures are illustrative synthetic examples across EN, DE, NL, FR, JA and ZH. Six redacted sensitive-topic placeholders are unsupported for classifier evaluation; 84 cases are evaluated. The private-session fixtures exercise a native paused-policy gate, not browser incognito capture end to end.

With the real pack, 55 cases exactly matched, 18 correctly returned empty, 10 missed expected evidence and one returned a partial or extra result. There were 67 relevant returned observations out of 78 expected observations; 20 semantic-paraphrase cases returned relevant evidence, and four expected observations appeared only in the semantic run. Lexical fallback had 26 exact matches, 32 correct empty results, 10 misses and 16 unexpected results under its separately specified labels. Neither mode returned results on the explicit no-match cases. No byte-bound, duplicate-ID, attribution or paused-policy invariant failures were recorded. These counts do not establish field accuracy or sensitive-classifier coverage. Real-world retrieval accuracy has not been established.

## Performance

At 10,000 synthetic atoms with 30 warm samples including process launch, indexed hybrid recall measured median 254.77 ms / p95 275.23 ms. All 10,000 atoms were indexed and all calls completed inside the 1,500 ms budget. The [benchmark record](benchmark-10k.md) includes the method, lexical comparison and refresh timing. The 800-atom process harness measured indexed recall at median 164.51 ms / p95 200.11 ms. Cold-cache performance and the specified dual-core/4 GiB reference machine were not measured.

## Limits

Full topic merge/split editing, coverage/relation selection, calibrated multilingual retrieval and precise inactivity-based session grouping are outside this release or require further validation. The paired-answer study, real skill-triggered execution in all six assistant clients, clean-machine matrix, macOS signing and permanent Firefox signing have not been completed. This release bundles only macOS Apple silicon executables. Chrome was tested using Google Chrome for Testing; managed browser policies and other Chromium browsers were not tested. Most browser harnesses inject synthetic observations into the queue or native bridge to verify delivery and storage; the real-site demo above separately verifies metadata capture on the listed public pages. The Firefox harness does not measure the batch timer or helper invocation count. Assistant command approvals depend on each local harness; cloud and sandboxed executors cannot use a vault they cannot access. The UI provides observed context and explicit corrections; it does not present unvalidated inferred interests as confirmed facts.

Machine-readable evidence is adjacent to this report: browser-test-results.json, firefox-test-results.json, native-test-results.json, install-test-results.json, skill-installer-test-results.json, model-pack-report.json, evaluation-results.json and performance-results.json. Actual UI captures are in screenshots/. The release workflow records artifact checksums and signing status.
