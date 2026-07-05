#!/bin/sh
set -eu

repo="${EMELA_REPO:-emela-lang/emela}"
install_dir="${EMELA_INSTALL_DIR:-$HOME/.emela/bin}"
version="${EMELA_VERSION:-}"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  *)
    echo "unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64 | aarch64)
    if [ "$os" = "apple-darwin" ]; then
      arch="aarch64"
    else
      echo "unsupported architecture for Linux: $(uname -m)" >&2
      exit 1
    fi
    ;;
  x86_64 | amd64)
    if [ "$os" = "unknown-linux-gnu" ]; then
      arch="x86_64"
    else
      echo "unsupported architecture for macOS: $(uname -m)" >&2
      exit 1
    fi
    ;;
  *)
    echo "unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

target="$arch-$os"
channel="${EMELA_CHANNEL:-stable}"

# curl for the GitHub REST API, sending GITHUB_TOKEN when set to lift the
# unauthenticated 60-requests/hour-per-IP rate limit. No -f: we want the JSON
# body even on HTTP errors so a 403 rate-limit message can be reported.
gh_api() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -sSL -H "Authorization: Bearer $GITHUB_TOKEN" "$@"
  else
    curl -sSL "$@"
  fi
}

# Resolve the download URL for the release to install:
#   EMELA_VERSION=x.y.z    -> that exact tag (either channel)
#   EMELA_CHANNEL=stable   -> the latest stable release (default)
#   EMELA_CHANNEL=nightly  -> the latest dev prerelease (published from `dev`)
#
# Pinned and stable installs build the URL from the tag alone, so they never
# touch the rate-limited api.github.com; only nightly needs to list releases.
if [ -n "$version" ]; then
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
  asset_url="https://github.com/$repo/releases/download/$tag/emela-$tag-$target.tar.gz"
elif [ "$channel" = "stable" ]; then
  # github.com/.../releases/latest 302-redirects to /releases/tag/<tag>; reading
  # the redirect target costs no API quota, unlike api.github.com/.../latest.
  tag="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/$repo/releases/latest" | sed -n 's#.*/releases/tag/##p')"
  if [ -z "$tag" ]; then
    echo "no stable emela release found in $repo yet" >&2
    echo "try a dev build with EMELA_CHANNEL=nightly, or pin one with EMELA_VERSION=x.y.z" >&2
    exit 1
  fi
  asset_url="https://github.com/$repo/releases/download/$tag/emela-$tag-$target.tar.gz"
elif [ "$channel" = "nightly" ]; then
  api_base="https://api.github.com/repos/$repo/releases"
  release_json="$(gh_api "$api_base?per_page=30" 2>/dev/null || true)"
  if printf '%s' "$release_json" | grep -q 'API rate limit exceeded'; then
    echo "GitHub API rate limit exceeded while looking up nightly builds for $repo" >&2
    echo "set GITHUB_TOKEN=<token> to raise the limit, or install a stable build (unset EMELA_CHANNEL)" >&2
    exit 1
  fi
  asset_url="$(printf '%s\n' "$release_json" \
    | awk -v target="$target" '
        /"prerelease": true/ { prerelease = 1 }
        /"prerelease": false/ { prerelease = 0 }
        /"browser_download_url":/ && prerelease && $0 ~ "emela-.*-" target "\\.tar\\.gz" {
          sub(/^.*"browser_download_url": *"/, "")
          sub(/".*$/, "")
          print
          exit
        }
      ')"
else
  echo "unknown EMELA_CHANNEL '$channel' (expected 'stable' or 'nightly')" >&2
  exit 1
fi

if [ -z "$asset_url" ]; then
  echo "could not find an emela release asset for $target in $repo" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

curl -fsSL "$asset_url" -o "$tmp_dir/emela.tar.gz"
tar -xzf "$tmp_dir/emela.tar.gz" -C "$tmp_dir"

mkdir -p "$install_dir"
find "$tmp_dir" -type f -name emela -perm -u+x -exec cp {} "$install_dir/emela" \; -quit

if [ ! -x "$install_dir/emela" ]; then
  echo "failed to install emela into $install_dir" >&2
  exit 1
fi

echo "installed $("$install_dir/emela" --version) to $install_dir/emela"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "add $install_dir to PATH to run emela directly" ;;
esac
