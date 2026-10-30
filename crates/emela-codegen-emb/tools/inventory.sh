#!/usr/bin/env bash
set -euo pipefail

crate_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
repo_dir=$(CDPATH= cd -- "$crate_dir/../.." && pwd)
if [ "$#" -eq 0 ]; then set -- all; fi
command=$1
shift
exec cargo run --quiet --locked --offline \
  --manifest-path "$crate_dir/tools/inventory/Cargo.toml" \
  --target-dir "${CARGO_TARGET_DIR:-$repo_dir/target/emb-inventory}" \
  -- "$command" --root "$repo_dir" "$@"
