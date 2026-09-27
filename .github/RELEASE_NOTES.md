# Serein v0.1.2

Your AI forgets what you researched. Serein gives a local assistant a way to find relevant pages and searches you chose to save, with their sources.

This release supports macOS on Apple silicon with Chrome and Firefox. The browser extension and the `serein-context` skill work together; the skill includes the local helper, model, and reader.

![Serein's first-run screen after linking](https://raw.githubusercontent.com/Maar10Herr/serein/v0.1.2/docs/screenshots/connected-first-run.png)

## Install

1. Download the [Chrome extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.2/serein-chrome-0.1.2-unsigned.zip) or [Firefox extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.2/serein-firefox-0.1.2-unsigned.zip). Extract the ZIP. In Chrome, open `chrome://extensions`, enable **Developer mode**, and select **Load unpacked**. In Firefox, open `about:debugging#/runtime/this-firefox`, select **Load Temporary Add-on**, and choose `manifest.json`.
2. Use your assistant's skill installer to install `serein-context` from [the Serein repository](https://github.com/Maar10Herr/serein). The [skill ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.2/serein-skills-0.1.2.zip) is available for inspection or installers that accept archives.
3. In the extension, open **Connections**, review the capture and recall choices, and select **Copy link instruction**. Paste it into the same local assistant to link the skill. Serein checks the connection automatically.

See the [installation guide](https://github.com/Maar10Herr/serein/blob/v0.1.2/docs/INSTALL.md) for the Codex Skills CLI command and full steps.

## What's new

- The empty dashboard now guides a newly connected user through the first useful recall question and shows the connection and search-readiness checks.
- The installed skill includes **Serein Doctor**, an on-demand diagnostic for browser pairing, native registration, vault access, model availability, skill discovery, and the recall setting. It reports saved setup state; it does not check whether a browser is currently open or prove that an assistant has executed the skill.
- Recall now uses a small recency and repeat-session signal when ordering its existing candidates. A constructed evaluation found modest gains on development journeys with mixed results on held-out Spanish and Japanese journeys; this is not evidence of overall or field accuracy. See the [test report](https://github.com/Maar10Herr/serein/blob/v0.1.2/docs/TEST_REPORT.md) for the scores and method.
- The repository adds a repeatable synthetic retrieval evaluation built from public-page metadata and hand-curated queries. Its timing and repeat-session values are constructed test inputs, not actual browsing activity.

The extension and macOS helper are unsigned; the helper is not notarized. Firefox's temporary installation expires when the browser restarts. Permanent Firefox installation requires Mozilla signing. The assistant must run on the same Mac as the extension.
