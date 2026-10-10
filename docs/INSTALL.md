# Install Serein v0.2.1

This release supports macOS with Apple silicon. Install the browser extension and skill on the same Mac. The skill includes the helper, reader, and model; its one-time link step registers the helper for your browser. Serein does not run a background service.

## 1. Install the extension

Download one extension from [the v0.2.1 release](https://github.com/Maar10Herr/serein/releases/tag/v0.2.1):

| Browser | Download | Load it |
| --- | --- | --- |
| Chrome | [Extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.1/serein-chrome-0.2.1-unsigned.zip) | Extract it. Open `chrome://extensions`, enable **Developer mode**, click **Load unpacked**, and select the folder containing `manifest.json`. |
| Firefox | [Extension ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.2.1/serein-firefox-0.2.1-unsigned.zip) | Extract it. Open `about:debugging#/runtime/this-firefox`, click **Load Temporary Add-on**, and select `manifest.json`. |

These ZIPs are unsigned. Firefox removes temporary add-ons when it restarts; load the extension again after a restart. Permanent Firefox installation requires a signed package, which this release does not provide. The macOS helper is also unsigned and not notarized.

## 2. Install the skill

Use your local assistant's GitHub skill installer to install `serein-context` from [Maar10Herr/serein](https://github.com/Maar10Herr/serein). If it asks for a folder, select `skills/serein-context`. The extension's **Connections** page also shows a ready-to-copy command for assistants using the [Skills CLI](https://github.com/vercel-labs/skills). For Codex, it is:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy
```

Installing a skill does not run its setup script automatically. No separate runtime archive is needed.

## 3. Link the skill to the extension

Open **Connections** in Serein. Review and accept the capture and assistant-recall choices, choose the local assistant where you installed the skill, and click **Copy link instruction**. Paste the copied instruction into that assistant. It tells the assistant to run the `scripts/connect.sh` file inside the installed skill, using the included JSON ticket as standard input. The script locates and verifies the bundled runtime, registers the browser helper for your user account, and reports the result. Serein checks the link automatically; no path or verification button is needed.

The ticket expires after 15 minutes. If linking takes longer, copy a new instruction. Keep the ticket on this Mac; do not put it in a public issue or a remote assistant. The assistant must be able to run commands on this Mac. Remote and isolated sessions cannot access the local vault.

The database is stored in your user application-data directory, outside the skill folder, so updating the skill does not replace your saved context. The helper starts for a browser batch or skill recall and exits after its reply.

If macOS blocks an unsigned binary, inspect the [source and checksums](https://github.com/Maar10Herr/serein/releases/tag/v0.2.1) before allowing it to run, or [build from source](DEVELOPMENT.md). Serein is licensed under [GPL-3.0-only](../LICENSE). The [test report](TEST_REPORT.md) distinguishes installed integrations from integrations executed inside real assistants.

**Upgrading from v0.2.0:** update the extension and reinstall the skill to replace the bundled helper. Your saved context is kept. The v0.2.1 helper continues to use vault schema 5. Older helpers refuse to open that schema. Keep the updated skill with the vault; replacing the extension alone does not update the helper.

For the first check, ask **“What did I find about [the topic I researched]?”** Use a topic visited after collection was enabled. An empty dashboard filter does not establish that the vault has no matching context.

If Serein does not connect or recall is unavailable, ask your local assistant to run **Serein Doctor** from the installed skill. It checks the browser pairing record, native host registration, local vault, model, skill detection, and recall setting, then gives a remedy for anything missing. Doctor checks local readiness; it cannot prove that a browser is currently open or that an assistant has executed the skill.
