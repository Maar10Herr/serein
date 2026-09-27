# Build Serein

The release includes browser extensions and a skill with the macOS Apple silicon runtime. Source builds on other platforms are possible, but this release does not package or validate them.

You need Rust, Node.js, pnpm, and Python 3. The included model pack is ready to use; rebuilding the model also requires NumPy and tokenizers.

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
python3 tools/bundle_skill.py
python3 tools/check_contracts.py
python3 tools/package.py
```

Extension builds are in `apps/extension/.output/`. `tools/bundle_skill.py` copies the release binaries, model, and dependency notices into the installable skill and writes runtime checksums. Packaging verifies the bundled version and checksums before writing the extension, skill, and source archives to `release/`. It scans them for local paths, tokens, and temporary files.

Set `SEREIN_SKILL_REPOSITORY` before building the extension and native runtime so both point to the published skill. `RUSTFLAGS` removes local filesystem paths from the binaries; packaging checks for them.

See the [test report](TEST_REPORT.md) for test scope and known limits. Protocol examples are in [`contracts/`](../contracts/); model provenance is in [`models/NOTICE.md`](../models/NOTICE.md).
