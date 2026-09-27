#!/bin/sh
set -eu
umask 077

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) platform=macos-arm64 ;;
  *) echo 'Serein currently includes a helper for macOS Apple silicon only.' >&2; exit 3 ;;
esac

skill_dir=$(CDPATH= cd "$(dirname "$0")/.." && pwd -P)
runtime="$skill_dir/runtime/$platform"
if [ ! -f "$runtime/serein" ] || [ ! -f "$runtime/serein-host" ] || [ ! -f "$runtime/model/manifest.json" ]; then
  echo 'The installed Serein skill is missing its local helper or model. Reinstall the skill from the release.' >&2
  exit 3
fi
(
  cd "$runtime"
  shasum -a 256 -c SHA256SUMS >/dev/null
) || { echo 'Serein skill runtime failed its checksum check. Reinstall the skill.' >&2; exit 3; }
chmod u+x "$runtime/serein" "$runtime/serein-host"
exec "$runtime/serein" setup --request-stdin --json
