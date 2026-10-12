#!/bin/sh
# Checks the syntax highlighting headlessly. Usage: run.sh [nvim|vim] (default: nvim)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
rtp=$(dirname "$here")
editor=${1:-nvim}
case "$editor" in
  nvim) exec nvim --headless -u NONE -i NONE -n \
          --cmd "set rtp^=$rtp" -c 'filetype plugin on' -c 'syntax on' -S "$here/check.vim" ;;
  vim)  exec vim -N -u NONE -i NONE -n -es \
          --cmd "set rtp^=$rtp" -c 'filetype plugin on' -c 'syntax on' -S "$here/check.vim" ;;
  *)    echo "unknown editor: $editor" >&2; exit 2 ;;
esac
