#!/bin/sh
set -eu

# Applies the branch-protection rulesets in .github/rulesets/ to the repository
# via the GitHub rulesets API. Idempotent: updates a ruleset in place if one with
# the same name already exists, otherwise creates it.
#
# Requirements: an authenticated `gh` (`gh auth login`) with admin rights on the
# repo, and `jq`. Run from anywhere:
#
#   sh scripts/apply-rulesets.sh
#
# Override the target repo with EMELA_REPO=owner/name.

repo="${EMELA_REPO:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"
dir="$(CDPATH= cd "$(dirname "$0")/../.github/rulesets" && pwd)"

for name in main dev; do
  json="$dir/$name.json"
  rulename="$(jq -r .name "$json")"
  id="$(gh api "repos/$repo/rulesets" --jq ".[] | select(.name==\"$rulename\") | .id" 2>/dev/null | head -n1 || true)"
  if [ -n "$id" ]; then
    echo "updating ruleset '$rulename' (#$id) on $repo"
    gh api --method PUT "repos/$repo/rulesets/$id" --input "$json" >/dev/null
  else
    echo "creating ruleset '$rulename' on $repo"
    gh api --method POST "repos/$repo/rulesets" --input "$json" >/dev/null
  fi
done

echo "done. review at: https://github.com/$repo/settings/rules"
