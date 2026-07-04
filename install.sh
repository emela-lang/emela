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
api_base="https://api.github.com/repos/$repo/releases"

# Resolve which release to install:
#   EMELA_VERSION=x.y.z    -> that exact tag (either channel)
#   EMELA_CHANNEL=stable   -> the latest stable release (default)
#   EMELA_CHANNEL=nightly  -> the latest dev prerelease (published from `dev`)
if [ -n "$version" ]; then
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
  release_json="$(curl -fsSL "$api_base/tags/$tag")"
  accept_any=1
elif [ "$channel" = "nightly" ]; then
  release_json="$(curl -fsSL "$api_base?per_page=30")"
  accept_any=0
elif [ "$channel" = "stable" ]; then
  # GitHub's "latest" excludes prereleases, so this is the newest stable tag.
  release_json="$(curl -fsSL "$api_base/latest" 2>/dev/null || true)"
  accept_any=1
  if ! printf '%s' "$release_json" | grep -q '"tag_name"'; then
    echo "no stable emela release found in $repo yet" >&2
    echo "try a dev build with EMELA_CHANNEL=nightly, or pin one with EMELA_VERSION=x.y.z" >&2
    exit 1
  fi
else
  echo "unknown EMELA_CHANNEL '$channel' (expected 'stable' or 'nightly')" >&2
  exit 1
fi

asset_url="$(printf '%s\n' "$release_json" \
  | awk -v target="$target" -v accept_any="$accept_any" '
      /"prerelease": true/ { prerelease = 1 }
      /"prerelease": false/ { prerelease = 0 }
      /"browser_download_url":/ && (prerelease || accept_any) && $0 ~ "emela-.*-" target "\\.tar\\.gz" {
        sub(/^.*"browser_download_url": *"/, "")
        sub(/".*$/, "")
        print
        exit
      }
    ')"

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
