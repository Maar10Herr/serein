# Serein v0.1.0

Serein lets local AI assistants recall browsing context you choose to save. This release supports **macOS with Apple silicon** and **Chrome or Firefox**.

## Install in three steps

1. **Install the extension.** Download one browser ZIP below. For Chrome, extract it and choose **Load unpacked** in `chrome://extensions`. For Firefox, extract it and select `manifest.json` through **Load Temporary Add-on** in `about:debugging#/runtime/this-firefox`.
2. **Install the skill.** In your local assistant, ask it to install `serein-context` from `https://github.com/Maar10Herr/serein` with its skill installer. The skill includes the helper, reader, and model. The extension also provides a copyable install request and a Skills CLI command.
3. **Link the skill.** Open **Connections** in Serein, review the capture and recall choices, then click **Copy link instruction**. Paste it into the local assistant where you installed the skill. Serein connects automatically when setup finishes.

| Download | Use |
| --- | --- |
| [Chrome extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.0/serein-chrome-0.1.0-unsigned.zip) | Load unpacked in Chrome. |
| [Firefox extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.0/serein-firefox-0.1.0-unsigned.zip) | Load as a temporary add-on in Firefox. |
| [Skill bundle](https://github.com/Maar10Herr/serein/releases/download/v0.1.0/serein-skills-0.1.0.zip) | Included for inspection or installers that accept a ZIP; normal installation uses the GitHub repository. |

There is no separate runtime download or database path to enter. The browser launches the helper for a batch of observations; the skill launches the reader only when the assistant needs context. Neither stays running between calls.

The extension packages and bundled macOS helper are unsigned. Firefox removes temporary add-ons after a restart; permanent installation requires a signed package. Local assistant execution requires the assistant to run on the same Mac. See the [install guide](https://github.com/Maar10Herr/serein/blob/v0.1.0/docs/INSTALL.md), [privacy details](https://github.com/Maar10Herr/serein/blob/v0.1.0/docs/PRIVACY.md), and [test report](https://github.com/Maar10Herr/serein/blob/v0.1.0/docs/TEST_REPORT.md) for supported behavior and limits.
