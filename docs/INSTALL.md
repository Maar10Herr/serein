# Install Serein

This beta runs on a Mac with Apple silicon. It needs one browser extension and the local runtime. Use an assistant that can run commands on the same Mac to complete pairing.

## Download

Get two files from [v0.1.0-beta.3](https://github.com/Maar10Herr/serein/releases/tag/v0.1.0-beta.3):

| Download | Purpose |
| --- | --- |
| [Chrome/Chromium extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.0-beta.3/serein-chrome-0.1.0-beta.3-unsigned.zip) **or** [Firefox extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.0-beta.3/serein-firefox-0.1.0-beta.3-unsigned.zip) | Choose the browser you use. |
| [macOS Apple silicon runtime](https://github.com/Maar10Herr/serein/releases/download/v0.1.0-beta.3/serein-macos-arm64-0.1.0-beta.3-unsigned.tar.gz) | Local helper, command-line tool, and model. Required for both browsers. |

The skill ZIP is an optional source bundle. Normal skill installation uses GitHub through the Skills CLI; there is no need to move files into a skill directory.

## Load the extension

**Chrome or Chromium:** Extract the extension ZIP. Open `chrome://extensions`, enable **Developer mode**, click **Load unpacked**, and select the extracted folder containing `manifest.json`.

**Firefox:** Extract the extension ZIP. Open `about:debugging#/runtime/this-firefox`, click **Load Temporary Add-on**, and select `manifest.json` in the extracted folder. Firefox removes temporary add-ons on restart, so load it again after restarting.

These beta packages are unsigned. You can inspect the [source](https://github.com/Maar10Herr/serein) before loading them.

## Connect the runtime

1. Extract the runtime archive. Keep `serein`, `serein-host`, and `model/` together in the extracted `serein` folder.
2. Open the extension's **Connections** page. Review the capture and assistant-recall choices, select your assistant, and decide whether to enable **Install the GitHub skill for these assistants when local setup runs**. Automatic skill installation needs `npx` or `pnpm` and access to GitHub.
3. Click **Copy local helper setup instructions**. Paste the copied text into an assistant running on this Mac, and give it the path to the extracted runtime folder. The pairing ticket in the text expires after 15 minutes; copy new instructions if it expires.
4. Review the assistant's command. It should feed the ticket to the extracted `serein` executable on standard input with `serein setup --request-stdin --json`. Setup registers the browser helper and reports skill installation separately.
5. Return to **Connections** and click **Verify connection**.

Keep the pairing ticket on your Mac. Do not put it in a public issue or send it to a remote assistant.

### Run setup yourself

Copy only the JSON object from the generated instructions into a local file named `ticket.json`. From the extracted runtime folder, run:

```sh
./serein setup --request-stdin --json < ticket.json
./serein doctor
```

Delete `ticket.json` after setup. If the ticket expired, copy a new one from **Connections**. The setup result reports both browser and skill status; **Verify connection** tests the browser link.

To install the skill yourself, use the [Skills CLI](https://github.com/vercel-labs/skills). For Codex:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy
```

The skill does not replace browser pairing. If macOS blocks the unsigned runtime, inspect it before allowing it to run, or [build from source](DEVELOPMENT.md).
