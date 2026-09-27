# Serein v0.1.0-beta.3

Serein gives local AI assistants a way to recall browser context you choose to save. This beta is for **macOS with Apple silicon** and **Chrome, Chromium, or Firefox**.

## Install

Download **one extension** and the **macOS runtime**:

| Download | Choose this if… |
| --- | --- |
| [Chrome/Chromium extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.0-beta.3/serein-chrome-0.1.0-beta.3-unsigned.zip) | You use Chrome or Chromium. |
| [Firefox extension](https://github.com/Maar10Herr/serein/releases/download/v0.1.0-beta.3/serein-firefox-0.1.0-beta.3-unsigned.zip) | You use Firefox. |
| [macOS Apple silicon runtime](https://github.com/Maar10Herr/serein/releases/download/v0.1.0-beta.3/serein-macos-arm64-0.1.0-beta.3-unsigned.tar.gz) | Required with either browser. |

1. Extract the extension ZIP. In Chrome, open `chrome://extensions`, enable **Developer mode**, and choose **Load unpacked**. In Firefox, open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on**, and select the extracted `manifest.json`.
2. Extract the runtime archive. Keep the `serein`, `serein-host`, and `model/` files together.
3. Open **Connections** in the extension. Select an assistant, review the capture and recall choices, and click **Copy local helper setup instructions**. Paste them into an assistant running on the same Mac and give it the extracted runtime folder path.
4. Click **Verify connection** in Serein after setup finishes.

The setup instructions include a pairing ticket that expires after 15 minutes. If it expires, copy new instructions from **Connections**. See the [install guide](https://github.com/Maar10Herr/serein/blob/main/docs/INSTALL.md) for manual setup.

## Skill and other downloads

During pairing, you can choose to install the `serein-context` skill directly from this GitHub repository. The standalone skill ZIP is for inspection or offline use; it is not needed for normal installation. For Codex, the direct command is:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --agent codex --global --yes --copy
```

The source archive and `SHA256SUMS` are attached below. The extension and native runtime are unsigned; Firefox removes temporary add-ons on restart. See [Privacy](https://github.com/Maar10Herr/serein/blob/main/docs/PRIVACY.md), [test results](https://github.com/Maar10Herr/serein/blob/main/docs/TEST_REPORT.md), and [LICENSE](https://github.com/Maar10Herr/serein/blob/main/LICENSE).
