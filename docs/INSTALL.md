# Install Serein

This release supports a Mac with Apple silicon. Install the browser extension and skill on the same Mac. The skill includes the helper, reader, and model; its one-time link step registers the helper for your browser. Serein does not run a background service.

## 1. Install the extension

Download one extension from [v0.1.1](https://github.com/Maar10Herr/serein/releases/tag/v0.1.1):

| Browser | Download | Load it |
| --- | --- | --- |
| Chrome | [Extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-chrome-0.1.1-unsigned.zip) | Extract it. Open `chrome://extensions`, enable **Developer mode**, click **Load unpacked**, and select the folder containing `manifest.json`. |
| Firefox | [Extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-firefox-0.1.1-unsigned.zip) | Extract it. Open `about:debugging#/runtime/this-firefox`, click **Load Temporary Add-on**, and select `manifest.json`. |

These ZIPs are unsigned. Firefox removes temporary add-ons when it restarts; load the extension again after a restart. Permanent Firefox installation requires a signed package, which this release does not provide. The macOS helper is also unsigned and not notarized.

## 2. Install the skill

In your local assistant, ask: **“Install the `serein-context` skill from `https://github.com/Maar10Herr/serein` using your skill installer.”** The installer should copy the skill from GitHub into that assistant's skill directory. For assistants using the [Skills CLI](https://github.com/vercel-labs/skills), the extension's **Connections** page shows a ready-to-copy command for the assistant you select. For Codex, it is:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy
```

Installing a skill does not run its setup script automatically. No separate runtime archive is needed.

## 3. Link the skill to the extension

Open **Connections** in Serein. Review and accept the capture and assistant-recall choices, choose the local assistant where you installed the skill, and click **Copy link instruction**. Paste the copied instruction into that assistant. It tells the assistant to run the `scripts/connect.sh` file inside the installed skill, using the included JSON ticket as standard input. The script locates and verifies the bundled runtime, registers the browser helper for your user account, and reports the result. Serein checks the link automatically while the page is open and in the background; no path or verification button is needed.

The ticket expires after 15 minutes. If linking takes longer, copy a new instruction. Keep the ticket on this Mac; do not put it in a public issue or a remote assistant. The assistant must be able to run commands on this Mac. Remote and isolated sessions cannot access the local vault.

The database is stored in your user application-data directory, outside the skill folder, so updating the skill does not replace your saved context. The helper starts for a browser batch or skill recall and exits after its reply.

If macOS blocks an unsigned binary, inspect the [source and checksums](https://github.com/Maar10Herr/serein/releases/tag/v0.1.1) before allowing it to run, or [build from source](DEVELOPMENT.md). The [test report](TEST_REPORT.md) distinguishes installed integrations from integrations executed inside real assistants.
