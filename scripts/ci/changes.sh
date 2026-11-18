#!/usr/bin/env bash
# Decides which groups of CI jobs a change needs, and writes `code`, `spec` and `grammar`
# (each `true` or `false`) to $GITHUB_OUTPUT.
#
# Reads EVENT (the event name) and, for `merge_group`, QUEUE_BASE (the base commit of the group).
# Any other event (a push) runs everything.
set -euo pipefail

out="${GITHUB_OUTPUT:-/dev/stdout}"

case "$EVENT" in
  # The checkout is the merge commit: its first parent is the base branch.
  pull_request) base=HEAD^1 ;;
  merge_group)
    git fetch --depth=1 origin "$QUEUE_BASE"
    base="$QUEUE_BASE"
    ;;
  *)
    printf 'code=true\nspec=true\ngrammar=true\n' >> "$out"
    exit 0
    ;;
esac

changed="$(git diff --name-only "$base" HEAD)"

# Writes `<name>=true` when a changed file matches the pattern (or, with -v, when one does not).
flag() {
  local name="$1" invert="" pattern
  shift
  if [ "$1" = -v ]; then invert=v; shift; fi
  pattern="$1"
  if echo "$changed" | grep -q${invert}E "$pattern"; then
    echo "$name=true" >> "$out"
  else
    echo "$name=false" >> "$out"
  fi
}

# Anything but the files that no build or test reads.
flag code -v '(\.md$|^docs/|^editors/|^LICENSE$|^\.github/ISSUE_TEMPLATE/|^\.github/CODEOWNERS$|^\.claude/skills/)'
# The specification, the proposals and the script that checks them.
flag spec '^(spec/|proposals/|scripts/spec\.py$)'
# The tree-sitter grammar, its editor queries, and the Emela files it must parse.
flag grammar '^editors/(tree-sitter-emela|zed|nvim)/|\.emel$'
