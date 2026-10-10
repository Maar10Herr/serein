# Serein v0.2.1

Serein brings selected browser research into a local assistant conversation through a small, source-linked context packet. This release improves bounded matching for exact values, comparisons, and unsegmented Japanese and Chinese text.

The helper runs only for a browser batch or assistant request and exits after replying; Serein adds no background service.

## Install on macOS with Apple silicon

1. Download the [Chrome extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.1/serein-chrome-0.2.1-unsigned.zip) or [Firefox extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.1/serein-firefox-0.2.1-unsigned.zip). Extract it. In Chrome, open `chrome://extensions`, enable **Developer mode**, and choose **Load unpacked**. In Firefox, open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on**, and select `manifest.json`.
2. Install `serein-context` from [Maar10Herr/serein](https://github.com/Maar10Herr/serein) with your assistant's GitHub skill installer. For Codex, use `npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy`. The [skill ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.1/serein-skills-0.2.1.zip) is available for archive-based installers.
3. Open **Connections** in the extension, review the capture and assistant-recall choices, choose your assistant, and select **Copy link instruction**. Paste the instruction into that assistant on this Mac to pair the bundled helper.

The Chrome and Firefox extensions are unsigned. Firefox treats the add-on as temporary and removes it when Firefox restarts, so load it again after a restart. The macOS helper is unsigned and not notarized. Serein is licensed under [GPL-3.0-only](https://github.com/Maar10Herr/serein/blob/main/LICENSE). See the [installation guide](https://github.com/Maar10Herr/serein/blob/main/docs/INSTALL.md) for details.

## What changed

- Better whole-word matching, including accents, short names, and model identifiers.
- Literal Japanese and Chinese phrases can now match inside unsegmented titles.
- Exact values preserve currencies, cents, signs, and unit dimensions. Unsupported numeric ranges return a clear warning.
- Two-model comparisons keep supported numeric qualifiers with the right model. Questions about personal purchases, choices, and time spent require confirmed evidence.
- Search results show the saved title when it supplies the matching evidence.

**Upgrading from v0.2.0:** update the extension and reinstall the skill to get the new helper. The vault stays on schema 5; your saved context is kept.

## Verification and limits

Chrome and Firefox passed the isolated end-to-end checks with the bundled helper. All 182 Rust tests and 58 extension tests passed, along with native installation and offline skill checks. Full results are recorded in the [test report](https://github.com/Maar10Herr/serein/blob/main/docs/TEST_REPORT.md). The [retrieval study](https://github.com/Maar10Herr/serein/blob/main/docs/validation/retrieval-study/README.md) retains the gains, remaining misses, and performance tradeoffs. Its small synthetic datasets do not establish accuracy on real browsing histories.

Setup adapters cover Claude Code, Codex, OpenCode, Hermes, OpenClaw, and generic local execution. Discovery and triggered recall inside the five named assistant clients remain unverified. General negation, word order, and broader multilingual matching remain limited.

Artifact checksums are included with the release.
