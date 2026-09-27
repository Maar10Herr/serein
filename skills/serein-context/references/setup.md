# Connect Serein on this device

The skill is published at [github.com/Maar10Herr/serein](https://github.com/Maar10Herr/serein/tree/main/skills/serein-context). Install it with a supported native skill installer. For the Skills CLI:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --global --yes --copy
```

The skill alone cannot register browser native messaging or reach a vault. Install the matching Serein browser extension and native runtime from the [release](https://github.com/Maar10Herr/serein/releases/tag/v0.1.0-beta.2). In the extension's **Connections** page, review consent, choose the assistants, and copy the short-lived pairing request. Pass that complete JSON through standard input to `serein setup --request-stdin --json` from the extracted native runtime. The opt-in **Install GitHub skill** choice can invoke the native Skills installer during setup and write this device's CLI path to `references/connection.md` in the installed skill. Users do not need to copy skill files into assistant directories.

Return to the extension and select **Verify connection**. Pairing verifies the browser helper; the assistant must still discover the installed skill. The CLI reports skill installation and local connection status separately, and neither is evidence that every assistant has executed a recall.

A source build can follow the repository README. Keep `serein`, `serein-host`, and `model/` together. Do not use a remote or sandboxed executor to bypass a local vault boundary.
