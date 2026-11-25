#!/usr/bin/env bash
# Fails a pull request that adds a proposal until the language's author has accepted it
# (proposals/README.md, "Decision").
#
# Merging a pull request that adds a proposal adopts it, and only the language's author adopts a
# proposal. The author comments `/accept` on the pull request; the `Proposal decision` workflow
# checks the proposal, comments a marker with the head commit it accepted, and runs this again.
# A commit pushed after that needs a new `/accept`.
#
# A pull request that adds a proposal also changes `spec/`: the proposal is reviewed as the change
# to the specification it makes.
#
# Reads GH_TOKEN, REPO, PR, HEAD_SHA and BOT (the login of the App that writes the marker). The
# checkout must be the merge commit of the pull request, with its first parent (the base branch)
# fetched.
set -euo pipefail

adding="$(git diff --name-only --diff-filter=A HEAD^1 HEAD -- 'proposals/*.md' \
  | grep -vE '^proposals/(README|TEMPLATE)\.md$|^proposals/.*/' || true)"
if [ -z "$adding" ]; then exit 0; fi

if ! git diff --name-only HEAD^1 HEAD -- spec/ | grep -qvx 'spec/history.md'; then
  echo "::error::the pull request adds $(echo $adding) but does not change spec/; apply the change to the specification in the same pull request"
  exit 1
fi

marker="<!-- proposal-accepted: $HEAD_SHA -->"
accepted="$(gh api --paginate "repos/$REPO/issues/$PR/comments" \
  --jq ".[] | select(.user.login == \"$BOT\") | .body" | grep -cxF "$marker" || true)"
if [ "$accepted" = 0 ]; then
  echo "::error::$(echo $adding) is not adopted yet; the language's author adopts it by commenting /accept on $HEAD_SHA"
  exit 1
fi
