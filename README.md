# Serein

**Your context. Only when it helps.**

Serein gives a paired AI assistant a small, source-backed context packet when you ask for it. A browser extension records permitted foreground tab titles, hostnames, recognized searches, and coarse activity after consent. A one-shot Rust helper stores that evidence in a local SQLite vault; a CLI retrieves relevant context on demand.

Serein does not read page bodies, import browser history, run a resident service, or send telemetry. Its interface supports English, German, Dutch, Simplified Chinese, Japanese, and Spanish, with light and dark themes.

![Serein context dashboard with synthetic example data](docs/screenshots/context-populated-light.png)

## Install

The [v0.1.0-beta.2 release](https://github.com/Maar10Herr/serein/releases/tag/v0.1.0-beta.2) contains a Chromium extension ZIP, a Firefox extension ZIP, a macOS arm64 native runtime with the compact model, and a skill bundle. Chrome and Firefox packages in this release are unsigned: load the Chromium build unpacked or load the Firefox build as a temporary add-on. Firefox removes temporary add-ons on restart. The native runtime is built for macOS arm64; other platforms can build from source.

1. Extract the browser ZIP. In Chromium, open the extension management page, enable Developer mode and choose **Load unpacked** on the extracted folder. In Firefox, open `about:debugging`, choose **This Firefox → Load Temporary Add-on**, and select its `manifest.json`.
2. Extract the macOS runtime archive. Keep `serein`, `serein-host`, and `model/` together. Open Serein → **Connections**, review the capture and disclosure choices, choose your assistant, and copy the pairing request.
3. Run `serein setup --request-stdin --json` from the extracted runtime, supplying the copied JSON request on standard input. Serein installs the native helper and registers the browser extension for this user account. Connections can install the GitHub skill with the native Skills installer when you opt in; it shows the actual installation result.
4. Select **Verify connection**. You can pause capture, limit it to selected sites, inspect evidence, correct context, or delete a site's evidence at any time.

Assistants that support the [Skills CLI](https://github.com/vercel-labs/skills) can also install the published skill directly, without manually copying files:

```sh
npx --yes skills add Maar10Herr/serein --skill serein-context --global --yes --copy
```

The skill uses the locally installed `serein` executable. Installing a skill alone does not pair the browser extension or install the native runtime. Review an assistant's permissions before enabling context recall.

## How it works

The extension captures metadata only from an eligible foreground tab after consent. Private windows, paused capture, excluded sites and known sensitive destinations are dropped before the host receives an event. The host processes one native message per invocation and exits. Its vault has bounded retention, site deletion epochs, source attribution and local-only search. Recall returns at most a small packet and distinguishes observations from facts you explicitly confirmed.

The available controls and caveats are described in [Privacy](docs/PRIVACY.md) and [Security](docs/SECURITY.md). Automatic sensitive filtering cannot guarantee that every sensitive title or query is recognized; selected-sites mode gives you tighter control. Deleting local evidence cannot retract context already disclosed to an assistant.

## Build from source

Requirements: Rust, Node.js and pnpm. Developer-only model conversion uses Python, NumPy and tokenizers. The model pack in the macOS release is already converted.

```sh
export SEREIN_SKILL_REPOSITORY=https://github.com/Maar10Herr/serein
export RUSTFLAGS="--remap-path-prefix=$HOME=/build --remap-path-prefix=$PWD=/src"
pnpm install --frozen-lockfile
pnpm --dir apps/extension check
pnpm --dir apps/extension test
pnpm --dir apps/extension build
pnpm --dir apps/extension build:firefox
cargo test --workspace --release
cargo build --workspace --release
python3 tools/check_contracts.py
python3 tools/package.py
```

`SEREIN_SKILL_REPOSITORY` configures the published skill source in both the extension and native runtime. `RUSTFLAGS` removes local home and checkout paths from compiled binaries; packaging checks for embedded home paths. Native setup verifies the repository URL and reports installation failures. `release/SHA256SUMS` verifies artifact integrity after downloading; unsigned checksums do not prove publisher identity.

The CLI exposes `recall`, `explain`, `doctor`, `refresh`, `adapters`, `export`, and `uninstall`. Assistants pass JSON through standard input rather than interpolating titles or queries into shell commands. `uninstall` retains vaults unless `erase_vaults` is explicitly true. It removes Serein-owned local connection files while preserving skill files owned by the external installer; remove that skill through the same installer when desired.

## Verification

Actual Chromium and Firefox native messaging was exercised on macOS arm64. [Test evidence and limitations](docs/TEST_REPORT.md) records the executed checks and retrieval results. The six assistant skill layouts were inspected and installed in isolated tests. Full end-to-end execution in every assistant, signed browser distribution, and a broader platform matrix are outside this beta's validation scope.

Serein's original code and skill are licensed under [GPL-3.0-only](LICENSE). The GPL permits commercial use and does not require users to pay a royalty. For terms outside the GPL, contact the repository maintainer about a separate commercial license. The bundled model and other third-party components retain their own licenses; their notices are included in the native archive and [license inventory](docs/THIRD_PARTY_LICENSES.json).
