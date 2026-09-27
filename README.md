# Serein

## Your AI forgets what you researched. Serein doesn't.

Pick up where your research left off. Serein gives your local AI assistant relevant pages and searches you chose to save, with sources, so you can keep moving instead of rebuilding the trail in every new chat.

Browse as usual. Serein keeps the useful context you allow. When you ask, your assistant can bring it back.

No browser-history import or upload. No page-body reading. No background service. Collection starts only after you opt in. The extension and helper work locally; when you ask for recall, selected context is sent to the assistant you paired, and may reach its model provider.

[Download](https://github.com/Maar10Herr/serein/releases/tag/v0.1.1) · [Install guide](docs/INSTALL.md) · [Privacy](docs/PRIVACY.md) · [Test results](docs/TEST_REPORT.md)

This preview shows a synthetic research journey through public pages about office-chair and workstation ergonomics. In an isolated Chrome profile, Serein captured five real tab titles or search terms after test consent; no account or source-page body was used. [View the dark-theme screenshot](docs/screenshots/research-demo-dark.png).

![Serein showing five cards captured during a synthetic public-site ergonomics research journey](docs/screenshots/research-demo-light.png)

## Install

The current release supports **macOS with Apple silicon**, using Chrome or Firefox.

1. **Install the extension.** Download the [Chrome ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-chrome-0.1.1-unsigned.zip) or [Firefox ZIP](https://github.com/Maar10Herr/serein/releases/download/v0.1.1/serein-firefox-0.1.1-unsigned.zip) and load it in your browser.
2. **Install the skill.** Ask your local assistant to install `serein-context` from `https://github.com/Maar10Herr/serein` using its skill installer. The skill includes the local helper, model, and reader.
3. **Link them.** Open **Connections** in the extension, review the capture and recall choices, and click **Copy link instruction**. Paste it into the local assistant where you installed the skill. Serein checks the connection automatically.

There is no separate runtime download or database path to enter. The [install guide](docs/INSTALL.md) has browser-specific steps and the Skills CLI command. The extension packages are unsigned; Firefox loads them as temporary add-ons that must be reloaded after a restart.

## Your controls

You decide which sites can be saved and whether an assistant can recall them. Serein records foreground tab metadata: title, hostname, recognized search term, and rough time in view. You can pause capture, use selected-sites mode, inspect saved evidence, correct a suggestion, or delete a site's data.

The browser and helper work locally. A recall packet sent to a paired assistant may then reach that assistant's model provider. See [Privacy](docs/PRIVACY.md) and [Security](docs/SECURITY.md) for details.

## Development

[Build and test Serein](docs/DEVELOPMENT.md) · [Read the test report](docs/TEST_REPORT.md)

Serein is licensed under [GPL-3.0-only](LICENSE). Licenses for the model and dependencies are included with the skill.
