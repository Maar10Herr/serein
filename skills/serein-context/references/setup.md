# Connect Serein

1. Install the Serein browser extension and install this skill from [GitHub](https://github.com/Maar10Herr/serein) with your assistant's skill installer.
2. In the extension's **Connections** page, review the capture and recall choices, then click **Copy link instruction**.
3. Paste that instruction into this local assistant. It contains a short-lived pairing ticket. Find this installed skill's directory from `SKILL.md` and run `sh scripts/connect.sh` with the JSON ticket on standard input. The script verifies the bundled macOS Apple silicon helper and model, registers the browser connection, and returns a setup result. Serein checks the connection automatically.

Keep the ticket on this device. It expires after 15 minutes. Do not include it in a command argument, public issue, or remote assistant request. The assistant must be able to execute commands on the same Mac as the browser. Installation in a hosted or isolated assistant does not grant access to the local vault.

The SQLite vault is stored in Serein's application-data directory. Skill updates do not replace it. The helper runs for one browser message or recall request and then exits.
