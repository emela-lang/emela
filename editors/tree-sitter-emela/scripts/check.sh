#!/bin/sh
# Checks the grammar: the generated parser is up to date, the corpus tests pass, every Emela file
# of the repository that the compiler accepts parses without errors, and the Zed queries are in
# sync. Needs the tree-sitter CLI and a C compiler.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/../.." && pwd)
cd "$here"

tree-sitter generate
if ! git diff --quiet -- src; then
  echo "src/ is out of date; run tree-sitter generate and commit the result" >&2
  git diff --stat -- src >&2
  exit 1
fi

tree-sitter test

# The compiler's error cases, the statement-level lexer inputs, and the formatter's input in the
# old form of anonymous functions (E0148) are not whole files that it accepts.
list=$(mktemp)
trap 'rm -f "$list"' EXIT
find "$root/examples" "$root/crates" "$root/tests" -name '*.emel' \
  -not -path '*/test_data/parser/err/*' -not -path '*/test_data/lexer/*' \
  -not -path '*/emela-fmt/test_data/old_lambdas.emel' > "$list"
tree-sitter parse --quiet --stat --paths "$list"

"$root/editors/zed/sync-queries.sh" --check
