#!/usr/bin/env bash
# Runs the compiler on every project in examples/.
#
#   scripts/ci/examples.sh <emela>                 check, test and `fmt --check` each example
#   scripts/ci/examples.sh <emela> --target <t>    only test each example on target <t>
set -euo pipefail

emela="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
shift
target=""
if [ "${1:-}" = --target ]; then target="$2"; fi

"$emela" --version

for dir in examples/*/; do
  echo "::group::$dir"
  (
    cd "$dir"
    if [ -n "$target" ]; then
      "$emela" test --target "$target"
    else
      "$emela" check && "$emela" test && "$emela" fmt --check
    fi
  )
  echo "::endgroup::"
done
