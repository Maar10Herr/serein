# Serein v0.1.3

Serein brings browser research you chose to save back into a conversation with a local AI assistant. This release makes the dashboard quieter: related research appears together, useful standalone observations remain visible, and the full activity list stays available when you need to inspect it.

## Install on a Mac with Apple silicon

1. Download the [Chrome extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.3/serein-chrome-0.1.3-unsigned.zip) or [Firefox extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.3/serein-firefox-0.1.3-unsigned.zip) and extract it. In Chrome, open `chrome://extensions`, turn on **Developer mode**, and choose **Load unpacked**. In Firefox, open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on**, and select the extracted `manifest.json`.
2. Install the `serein-context` skill from [Maar10Herr/serein](https://github.com/Maar10Herr/serein) with your local assistant's skill installer. For Codex, use `npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy`. The [skill ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.3/serein-skills-0.1.3.zip) is also available for archive-based installers.
3. In the extension, open **Connections**, choose your assistant, and select **Copy link instruction**. Paste it into that assistant. The installed skill registers the included helper and Serein checks the connection automatically. No database path or separate runtime download is needed.

Firefox temporary add-ons need to be reloaded after a browser restart. The extensions and macOS helper are unsigned. See the [installation guide](https://github.com/Maar10Herr/serein/blob/main/docs/INSTALL.md) for details.

## What changed

- Research groups and selected observations now lead the dashboard; raw activity is one click away. Corrections and forgetting update what can appear in groups and assistant recall.
- Retrieval combines exact terms, a multilingual local model, and source-specific evidence. A bounded recency signal can reorder close matches but cannot make unrelated activity relevant.
- English, German, Dutch, Spanish, Japanese, and Simplified Chinese interface text is included. The settings view follows the system light or dark theme until you choose an override.

## Verification and limits

Chrome and Firefox end-to-end browser checks passed on isolated profiles, including native pairing, ingestion, privacy controls, and theme behavior. On a fresh **constructed** 35-question retrieval holdout, macro Recall@6 was **0.6522** for the 23 questions with expected evidence; all **12/12** expected-empty questions returned no context. A separate constructed dashboard holdout had 24 true positives, one false positive, 12 true negatives, and five false negatives. These are synthetic labels, not measurements of real-user accuracy. Some non-empty retrieval questions still miss evidence or include extra results.

The skill targets Claude Code, Codex, OpenCode, Hermes, OpenClaw, and generic local execution. Codex skill installation and the generic setup/reader path have been checked; automatic discovery and triggered recall inside those assistant clients have not been verified. The [test report](https://github.com/Maar10Herr/serein/blob/main/docs/TEST_REPORT.md) records the exact scope.
