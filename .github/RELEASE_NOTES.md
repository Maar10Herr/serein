# Serein v0.1.1

Serein gives local assistants access to browsing context you choose to save. This release supports macOS on Apple silicon with Chrome or Firefox.

## Install

1. Download the [Chrome extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-chrome-0.1.1-unsigned.zip) or [Firefox extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-firefox-0.1.1-unsigned.zip). Extract the ZIP. In Chrome, open `chrome://extensions`, enable **Developer mode**, and choose **Load unpacked**. In Firefox, open `about:debugging#/runtime/this-firefox` and choose **Load Temporary Add-on** with `manifest.json`.
2. Ask your local assistant to install the `serein-context` skill from `https://github.com/Maar10Herr/serein` with its native skill installer. The skill includes the helper, model, and reader; no separate runtime download is needed.
3. Open **Connections** in the extension, review the capture settings, and choose **Copy link instruction**. Paste it into the same local assistant. Serein checks the connection automatically.

The [install guide](https://github.com/Maar10Herr/serein/blob/v0.1.1/docs/INSTALL.md) has the full steps. The [skill ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-skills-0.1.1.zip) is available for inspection or installers that accept archives.

## Changes

- Retrieval now ranks lexical, semantic, and confirmed evidence separately before combining them. Exact values such as product identifiers, dates, prices, and measurements must match in full.
- Model changes rebuild topic assignments deterministically. Deletion updates affected topics, and topic labels follow the current evidence.
- Recall reserves time for retrieval and opens the model once per request. Local file and database failures have distinct, actionable error codes.
- The extension follows the system appearance by default. Explicit light and dark choices remain saved.

An isolated 10,000-atom vault returned hybrid recall in 255 ms median and 275 ms p95 across 30 warm calls on the tested Apple silicon Mac. See the [benchmark method](https://github.com/Maar10Herr/serein/blob/v0.1.1/docs/benchmark-10k.md) and [test report](https://github.com/Maar10Herr/serein/blob/v0.1.1/docs/TEST_REPORT.md) for scope and limits.

The extension and macOS helper are unsigned. Firefox's temporary installation expires when the browser restarts; permanent installation requires Mozilla signing. The assistant must run on the same Mac as the extension.
