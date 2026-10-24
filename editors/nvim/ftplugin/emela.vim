" Vim/Neovim filetype plugin for the Emela language (0.20).
" Language:  Emela
" Filenames: *.emel

if exists("b:did_ftplugin")
  finish
endif
let b:did_ftplugin = 1

let s:cpo_save = &cpo
set cpo&vim

" Line comments only (spec 2.2). `##` is a doc comment and continues as `##`.
setlocal commentstring=#\ %s
setlocal comments=:##,:#
setlocal formatoptions-=t formatoptions+=croql

" `emela fmt` indents by two spaces and never uses tabs.
setlocal expandtab shiftwidth=2 softtabstop=2 tabstop=8

" `gf` on a module path: `Http.Client` is `src/http/client.emel`, `JsonCodec` is `json_codec.emel`
" (spec 4.1).
setlocal suffixesadd=.emel
setlocal include=^\\s*import\\s\\+
setlocal includeexpr=EmelaModuleToPath(v:fname)

function! EmelaModuleToPath(name) abort
  " Drop an import list such as `.{Json, decode}`, then convert each segment.
  let l:path = substitute(a:name, '\.\=\%({.*\)\=$', '', '')
  let l:path = substitute(l:path, '\l\zs\(\u\)', '_\l\1', 'g')
  let l:path = substitute(l:path, '\d\zs\(\u\)', '_\l\1', 'g')
  return tolower(substitute(l:path, '\.', '/', 'g'))
endfunction

" A project's source root is `src/` beside the nearest `Pome.toml` (spec 4.1).
let s:pome = findfile('Pome.toml', expand('%:p:h') . ';')
if !empty(s:pome)
  execute 'setlocal path^=' . fnameescape(fnamemodify(s:pome, ':p:h') . '/src')
endif
unlet s:pome

let b:undo_ftplugin = "setlocal commentstring< comments< formatoptions< expandtab< shiftwidth<"
      \ . " softtabstop< tabstop< suffixesadd< include< includeexpr< path<"

" In Neovim, highlight through the tree-sitter parser when it is installed (see README.md).
" Without it, or with `g:emela_treesitter` set to 0, syntax/emela.vim highlights as before.
if has('nvim') && get(g:, 'emela_treesitter', 1)
  lua pcall(vim.treesitter.start)
  let b:undo_ftplugin .= " | lua pcall(vim.treesitter.stop)"
endif

let &cpo = s:cpo_save
unlet s:cpo_save
