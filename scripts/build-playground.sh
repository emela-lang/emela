#!/usr/bin/env bash
# Builds the compiler for the browser (crates/emela-playground) and writes it to the output
# directory (default: target/playground):
#
#   emela_playground.js        the wasm-bindgen glue (`--target web`): `init`, `check`, `format`, `compile`
#   emela_playground_bg.wasm   the compiler
#
# Needs the wasm32-unknown-unknown target of rustup and the wasm-bindgen CLI of the same version
# as the wasm-bindgen crate in Cargo.lock. wasm-opt (binaryen) shrinks the module when it is on
# PATH; without it the module is larger but works the same.
#
# Usage: scripts/build-playground.sh [OUT_DIR]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${1:-$root/target/playground}"

want="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3; exit }' "$root/Cargo.lock")"
have="$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }' || true)"
if [ "$have" != "$want" ]; then
  echo "error: wasm-bindgen $want is needed (found: ${have:-none})" >&2
  echo "       cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi

cargo build --manifest-path "$root/Cargo.toml" --release --locked \
  -p emela-playground --target wasm32-unknown-unknown
wasm="$root/target/wasm32-unknown-unknown/release/emela_playground.wasm"

mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name emela_playground "$wasm"

if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz --all-features "$out/emela_playground_bg.wasm" -o "$out/emela_playground_bg.wasm"
else
  echo "note: wasm-opt not found; the module is not shrunk" >&2
fi
ls -l "$out"
