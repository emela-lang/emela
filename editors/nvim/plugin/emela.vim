" Neovim plugin for the Emela language (0.20).
"
" `emela lsp` marks capability values (`log = use Logger`, spec 8.2) with semantic tokens of type
" `variable` and modifier `capability`. Underline them by default. `highlight default` lets a
" color scheme or your config choose otherwise. Vim has no semantic tokens, so nothing to do.

if !has('nvim') || exists('g:loaded_emela')
  finish
endif
let g:loaded_emela = 1

function! s:Highlight() abort
  highlight default @lsp.typemod.variable.capability.emela gui=underline cterm=underline
endfunction

call s:Highlight()

" A color scheme clears every group when it loads.
augroup emela_capability
  autocmd!
  autocmd ColorScheme * call s:Highlight()
augroup END
