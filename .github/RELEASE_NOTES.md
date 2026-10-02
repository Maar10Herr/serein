# Serein v0.2.0

Bring the research you saved back into the conversation. Serein keeps selected browser metadata locally and gives a paired assistant a small, source-linked answer to the current question.

## Install on a Mac with Apple silicon

1. Download the [Chrome extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.0/serein-chrome-0.2.0-unsigned.zip) or [Firefox extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.0/serein-firefox-0.2.0-unsigned.zip) and extract it. In Chrome, open `chrome://extensions`, enable **Developer mode**, and choose **Load unpacked**. In Firefox, open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on**, and select `manifest.json`.
2. Install `serein-context` from [Maar10Herr/serein](https://github.com/Maar10Herr/serein) with your assistant's skill installer. For Codex: `npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy`. The [skill ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.0/serein-skills-0.2.0.zip) is available for archive-based installers.
3. Open **Connections** in the extension, choose your assistant, and select **Copy link instruction**. Paste it into that assistant on this Mac. The installed skill registers its bundled helper and Serein checks the connection automatically.

No separate runtime download or database path is needed. The helper starts for a request and exits after replying. Firefox temporary add-ons must be reloaded after a restart. The extensions and helper are unsigned. [Installation guide](https://github.com/Maar10Herr/serein/blob/main/docs/INSTALL.md).

## What changed

- Recall applies eligibility before candidate limits, avoids redundant packet entries, and can retain evidence for both models in a supported comparison.
- Dashboard refresh preserves newer corrections and hides context while a privacy change is unconfirmed. Filters clearly describe the displayed view.
- Correction drafts belong to the card that opened them. Dialogs contain keyboard focus and restore it when closed.
- Topic refresh reuses valid vectors and maintains bounded centroid caches with reference recomputation as a fallback.
- Activity counts are labeled as fixed 30-minute windows. Research groups remain provisional summaries of activity.
- SHA verification uses hardware acceleration while still rejecting corrupted model files.

The vault upgrades to schema 5. Older readers refuse to open it; keep the updated skill with an upgraded vault.

## Verification and limits

Chrome and Firefox passed the isolated end-to-end checks with the packaged helper. The 10,000-record synthetic test completed all recalls within budget; warm recall measured 155 ms median and 161 ms p95 on the tested Mac. The [test report](https://github.com/Maar10Herr/serein/blob/main/docs/TEST_REPORT.md) includes raw results, retained retrieval misses, and the [light](https://github.com/Maar10Herr/serein/blob/main/docs/screenshots/research-preview-light.png) and [dark](https://github.com/Maar10Herr/serein/blob/main/docs/screenshots/research-preview-dark.png) constructed previews. Synthetic results do not establish real-user accuracy. Broader multilingual query tuning, general negation and word-order understanding remain open.

The skill targets Claude Code, Codex, OpenCode, Hermes, OpenClaw and generic local execution. Packaged integration paths and local setup checks are distinct from discovery and triggered recall inside each assistant client. Remote executors cannot access this local vault.

Serein is [GPL-3.0-only](https://github.com/Maar10Herr/serein/blob/main/LICENSE). Artifact checksums are included with the release.
