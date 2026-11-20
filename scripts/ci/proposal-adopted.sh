#!/usr/bin/env bash
# Fails a pull request that adopts a proposal until the language's author has commented `/accept`
# on it (proposals/README.md, "Decision").
#
# Merging a pull request that adds a proposal adopts it, and only the language's author adopts a
# proposal. A pull request cannot be approved by its own author, so the author comments `/accept`
# instead; the `Proposal decision` workflow then pushes `status: accepted`, which runs this again.
#
# Reads GH_TOKEN, REPO, PR and AUTHOR. The checkout must be the merge commit of the pull request,
# with its first parent (the base branch) fetched.
set -euo pipefail

status() { sed -n 's/^status:[[:space:]]*\([a-z]*\).*/\1/p' | head -n1; }

adopting=""
for f in $(git diff --name-only --diff-filter=AM HEAD^1 HEAD -- 'proposals/*.md'); do
  case "$f" in proposals/README.md | proposals/TEMPLATE.md | proposals/*/*) continue ;; esac
  # A new proposal is not in the base branch; `git show` fails, which `pipefail` would make fatal.
  before="$( (git show "HEAD^1:$f" 2>/dev/null || true) | status)"
  # A proposal that was already adopted is only being edited (a typo, `implemented`).
  case "$before" in accepted | implemented) continue ;; esac
  after="$(status < "$f")"
  if [ "$after" != accepted ]; then
    echo "::error file=$f::not adopted yet (status: $after); the author adopts it by commenting /accept"
    exit 1
  fi
  adopting="$adopting $f"
done
if [ -z "$adopting" ]; then exit 0; fi

# `status: accepted` can be written by anyone; the comment can be written only by the author.
accepted="$(gh api --paginate "repos/$REPO/issues/$PR/comments" \
  --jq ".[] | select(.user.login == \"$AUTHOR\") | .body | split(\"\n\")[0] | gsub(\"^\\\\s+|\\\\s+$\"; \"\")" \
  | grep -cx '/accept' || true)"
if [ "$accepted" = 0 ]; then
  echo "::error::$adopting is accepted, but $AUTHOR has not commented /accept on this pull request"
  exit 1
fi
