#!/bin/sh
set -eu
case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) platform=macos-arm64 ;;
  *) echo 'Serein recall currently supports macOS Apple silicon only.' >&2; exit 3 ;;
esac
skill_dir=$(CDPATH= cd "$(dirname "$0")/.." && pwd -P)
runtime="$skill_dir/runtime/$platform"
if [ ! -f "$runtime/serein" ]; then
  echo 'Serein skill runtime missing. Reinstall the skill.' >&2
  exit 3
fi
chmod u+x "$runtime/serein"
exec "$runtime/serein" recall --request-stdin --json
