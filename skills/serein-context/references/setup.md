# Connect Serein on this device

Install the skill from [GitHub](https://github.com/Maar10Herr/serein/tree/main/skills/serein-context) with your assistant's skill installer. For Codex with the Skills CLI:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy
```

The skill also needs the browser extension and local runtime from the [current release](https://github.com/Maar10Herr/serein/releases/tag/v0.1.0-beta.3). Follow the [install guide](https://github.com/Maar10Herr/serein/blob/main/docs/INSTALL.md) to load the extension and pair the runtime. Keep `serein`, `serein-host`, and `model/` together.

In the extension's **Connections** page, select an assistant and copy the local helper setup instructions. Run setup on the same device, passing the complete pairing JSON to `serein setup --request-stdin --json` on standard input. The ticket expires after 15 minutes. Return to **Connections** and click **Verify connection**.

Installing the skill and pairing the browser are separate steps. The CLI reports their status separately. An assistant must still discover the installed skill before it can recall saved context.
