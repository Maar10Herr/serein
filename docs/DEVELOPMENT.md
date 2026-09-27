# Build Serein

The release includes browser extensions and a macOS Apple silicon runtime. Build from source for development or another platform.

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
python3 tools/check_contracts.py
python3 tools/package.py
```

Extension builds are in `apps/extension/.output/`. Packaging writes distribution archives and `SHA256SUMS` to `release/` and scans them for local paths, tokens, and temporary files. The native archive contains the executable for the machine on which you build it.

Set `SEREIN_SKILL_REPOSITORY` before building the extension and native runtime so both point to the published skill. `RUSTFLAGS` removes local filesystem paths from the binaries; packaging checks for them.

See the [test report](TEST_REPORT.md) for test scope and known limits. Protocol examples are in [`contracts/`](../contracts/); model provenance is in [`models/NOTICE.md`](../models/NOTICE.md).
