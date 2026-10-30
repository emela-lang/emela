#!/usr/bin/env bash
set -euo pipefail

crate_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec bash "$crate_dir/tools/inventory.sh" definitions "$@"
