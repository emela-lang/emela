#!/bin/sh
# Checks the syntax highlighting headlessly. Usage: run.sh [nvim|vim|treesitter] (default: nvim)
#
# `treesitter` builds the parser in editors/tree-sitter-emela with the tree-sitter CLI and checks
# the tree-sitter highlighting in Neovim instead of syntax/emela.vim.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
rtp=$(dirname "$here")
editor=${1:-nvim}
case "$editor" in
  nvim) exec nvim --headless -u NONE -i NONE -n --cmd 'let g:emela_treesitter = 0' \
          --cmd "set rtp^=$rtp" -c 'filetype plugin on' -c 'syntax on' -S "$here/check.vim" ;;
  treesitter)
    parser=$(mktemp -d)
    trap 'rm -rf "$parser"' EXIT
    mkdir -p "$parser/parser"
    tree-sitter build -o "$parser/parser/emela.so" "$(dirname "$rtp")/tree-sitter-emela"
    nvim --headless -u NONE -i NONE -n --cmd "set rtp^=$rtp,$parser" \
      -c 'filetype plugin on' -c "luafile $here/treesitter.lua" ;;
  vim)  exec vim -N -u NONE -i NONE -n -es \
          --cmd "set rtp^=$rtp" -c 'filetype plugin on' -c 'syntax on' -S "$here/check.vim" ;;
  *)    echo "unknown editor: $editor" >&2; exit 2 ;;
esac
