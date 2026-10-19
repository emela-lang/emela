" Checks the syntax groups at chosen positions, and that the example files show no Error
" highlighting. Run through run.sh.

let s:here = expand('<sfile>:p:h')
let s:root = fnamemodify(s:here, ':h:h:h')
let s:failures = []
let s:checked = 0

function! s:Open(path) abort
  execute 'silent edit! ' . fnameescape(a:path)
  if &filetype !=# 'emela'
    call add(s:failures, a:path . ': filetype is "' . &filetype . '", not "emela"')
  endif
endfunction

" On the first line containing `line`, the `nth` occurrence of `token` starts in `group`.
function! s:Expect(line, token, group, ...) abort
  let l:nth = a:0 ? a:1 : 1
  let l:lnum = search('\V' . escape(a:line, '\'), 'nw')
  let l:where = expand('%:t') . ': `' . a:line . '` `' . a:token . '`'
  if l:lnum == 0
    call add(s:failures, l:where . ': line not found')
    return
  endif
  let l:col = -1
  for _ in range(l:nth)
    let l:col = stridx(getline(l:lnum), a:token, l:col + 1)
  endfor
  if l:col < 0
    call add(s:failures, l:where . ': token not found')
    return
  endif
  let s:checked += 1
  let l:actual = synIDattr(synID(l:lnum, l:col + 1, 1), 'name')
  if l:actual !=# a:group
    call add(s:failures, printf('%s (#%d, line %d): expected %s, got %s',
          \ l:where, l:nth, l:lnum, a:group, empty(l:actual) ? '(none)' : l:actual))
  endif
endfunction

" No character of the buffer is highlighted through Error.
function! s:ExpectNoErrors() abort
  let l:error = hlID('Error')
  for l:lnum in range(1, line('$'))
    let l:text = getline(l:lnum)
    for l:col in range(1, len(l:text))
      let l:id = synID(l:lnum, l:col, 1)
      if synIDtrans(l:id) == l:error
        call add(s:failures, printf('%s:%d:%d: %s on `%s`', expand('%:.'), l:lnum, l:col,
              \ synIDattr(l:id, 'name'), l:text))
        break
      endif
    endfor
  endfor
  let s:checked += 1
endfunction

" ---------------------------------------------------------------------------
call s:Open(s:here . '/fixture.emel')

" Comments
call s:Expect('# Places', '#', 'emelaComment')
call s:Expect('## A documented', '##', 'emelaDocComment')
call s:Expect('## A documented', 'TODO', 'emelaTodo')

" Imports and declarations
call s:Expect('import Http.Client', 'import', 'emelaInclude')
call s:Expect('import Http.Client', 'Client', 'emelaModulePath')
call s:Expect('import Data.Json', 'Json', 'emelaType', 2)
call s:Expect('pub opaque type', 'pub', 'emelaStorageClass')
call s:Expect('pub opaque type', 'opaque', 'emelaStorageClass')
call s:Expect('pub opaque type', 'type', 'emelaStructure')
call s:Expect('pub opaque type', 'Email', 'emelaTypeDecl')
call s:Expect('pub opaque type', 'String', 'emelaType')
call s:Expect('  derive Eq', 'derive', 'emelaContextual')
call s:Expect('type Pair', 'Pair', 'emelaTypeDecl')
call s:Expect('type Pair', 'A', 'emelaTypeParamDecl')
call s:Expect('type Pair', 'B', 'emelaTypeParamDecl')
call s:Expect('type Pair', 'Eq', 'emelaType')
call s:Expect('type Pair', '+', 'emelaOperator')
call s:Expect('type Pair', 'A, s', 'emelaTypeParam')
call s:Expect('enum Shape', 'Shape', 'emelaTypeDecl')
call s:Expect('Circle(radius', 'Circle', 'emelaConstructor')
call s:Expect('Circle(radius', 'Float', 'emelaType')
call s:Expect('error NotFound', 'error', 'emelaStructure')
call s:Expect('error NotFound', 'NotFound', 'emelaTypeDecl')
call s:Expect('const MAX_SIZE', 'MAX_SIZE', 'emelaConstDecl')
call s:Expect('const MAX_SIZE', '1_000', 'emelaNumber')

" Annotations
call s:Expect('@external', '@external', 'emelaAnnotation')
call s:Expect('@external', '"Math.abs"', 'emelaString')
call s:Expect('@intrinsic', '@intrinsic', 'emelaAnnotation')
call s:Expect('fn abs', 'abs', 'emelaFuncDecl')

" Effects and handlers
call s:Expect('effect Users', 'effect', 'emelaEffectDecl')
call s:Expect('effect Users', 'Users', 'emelaTypeDecl')
call s:Expect('suspend fn find', 'suspend', 'emelaStorageClass')
call s:Expect('suspend fn find', 'find', 'emelaFuncDecl')
call s:Expect('suspend fn find', 'fails', 'emelaContextual')
call s:Expect('suspend fn find', 'NotFound', 'emelaType')
call s:Expect('suspend fn find', '|', 'emelaOperator')
call s:Expect('suspend fn find', 'DbError', 'emelaType')
call s:Expect('handler PostgresUsers', 'handler', 'emelaEffectDecl')
call s:Expect('handler PostgresUsers', 'PostgresUsers', 'emelaTypeDecl')
call s:Expect('handler PostgresUsers', 'Users', 'emelaType', 2)
call s:Expect('  init {', 'init', 'emelaContextual')
call s:Expect('  release {', 'release', 'emelaContextual')
call s:Expect('PostgresUsers(conn: connect', 'PostgresUsers', 'emelaConstructor')
call s:Expect('PostgresUsers(conn: connect', 'connect', 'emelaFunctionCall')
call s:Expect('close(self', 'self', 'emelaSelf')
call s:Expect('layer AppLive', 'layer', 'emelaEffectDecl')
call s:Expect('layer AppLive', 'ConsoleLogger', 'emelaType')
call s:Expect('impl Shape', 'impl', 'emelaKeyword')
call s:Expect('impl Shape', 'Show', 'emelaType')

" fails A | B and use { A, R }
call s:Expect('fn run[', 'run', 'emelaFuncDecl')
call s:Expect('fn run[', 'R', 'emelaTypeParamDecl')
call s:Expect('fn run[', 'fn', 'emelaKeyword', 2)
call s:Expect('fn run[', 'fails', 'emelaContextual')
call s:Expect('fn run[', 'A |', 'emelaTypeParam')
call s:Expect('fn run[', '| B', 'emelaOperator')
call s:Expect('fn run[', 'B use', 'emelaTypeParam')
call s:Expect('fn run[', 'use', 'emelaEffect')
call s:Expect('fn run[', 'A, R }', 'emelaTypeParam')
call s:Expect('fn run[', 'R }', 'emelaTypeParam')
call s:Expect('fn run[', '->', 'emelaOperator')
call s:Expect('with AppLive', 'with', 'emelaEffect')

" Numbers and literals
call s:Expect('hex = ', '0xFF_ff', 'emelaNumber')
call s:Expect('hex = ', '0b1010', 'emelaNumber')
call s:Expect('hex = ', '3.14', 'emelaFloat')
call s:Expect('hex = ', '1.0e9', 'emelaFloat')
call s:Expect('hex = ', '1..2', 'emelaNumber')
call s:Expect('hex = ', '..', 'emelaOperator')
call s:Expect('bad = ', '1e9', 'emelaNumberError')
call s:Expect('bad = ', '0XFF', 'emelaNumberError')
call s:Expect('unit = ', '()', 'emelaUnit')
call s:Expect('flags = ', 'True', 'emelaBoolean')
call s:Expect('flags = ', '&&', 'emelaOperator')
call s:Expect('flags = ', '!=', 'emelaOperator')
call s:Expect('flags = ', '|>', 'emelaOperator')
call s:Expect('flags = ', '||', 'emelaOperator')

" Strings, escapes and interpolation
call s:Expect('escaped = ', '\t', 'emelaEscape')
call s:Expect('escaped = ', '\u{1F600}', 'emelaEscape')
call s:Expect('escaped = ', '\"', 'emelaEscape')
call s:Expect('escaped = ', '\#', 'emelaEscape')
call s:Expect('escaped = ', '{not}', 'emelaString')
call s:Expect('escaped = ', '\q', 'emelaEscapeError')
call s:Expect('nested = ', '#{', 'emelaInterpDelim')
call s:Expect('nested = ', 'f(', 'emelaFunctionCall')
call s:Expect('nested = ', '1', 'emelaNumber')
call s:Expect('nested = ', '}', 'emelaInterpDelim', 2)
call s:Expect('nested = ', ' b ', 'emelaString')
call s:Expect('nested = ', 'in ', 'emelaString')
call s:Expect('nested = ', 'y', 'emelaInterp')
call s:Expect('nested = ', ' ner', 'emelaString')
call s:Expect('nested = ', ' c', 'emelaString')
call s:Expect('commented = ', '# }', 'emelaComment')
call s:Expect('  } b"', '}', 'emelaInterpDelim')
call s:Expect('  } b"', 'b', 'emelaString')
call s:Expect('multi = ', '"""', 'emelaString')
call s:Expect('line #{ name }', 'line', 'emelaString')
call s:Expect('line #{ name }', 'name', 'emelaInterp')
call s:Expect('"quoted" ## not', '"quoted"', 'emelaString')
call s:Expect('"quoted" ## not', '##', 'emelaString')
call s:Expect('after = 1', '1', 'emelaNumber')

" Contextual words used as names
call s:Expect('for = 2', 'for', '')
call s:Expect('fails = for', 'fails', '')
call s:Expect('fails = for', 'for', '')

" Expressions
call s:Expect('r = x escape', 'escape', 'emelaException')
call s:Expect('r = x escape', 'NotFound', 'emelaConstructor')
call s:Expect('r = x escape', 'fail', 'emelaException')
call s:Expect('assert self', 'assert', 'emelaStatement')
call s:Expect('assert self', 'Self', 'emelaSelf')

" `await` is not reserved (spec 3.3): `Fiber.await` and `Deferred.await` are functions.
call s:Expect('waited = ', 'await', 'emelaFunctionCall')
call s:Expect('waited = ', 'await', 'emelaFunctionCall', 2)
call s:Expect('awaiting = ', 'await', '')

" Postfix match (spec 6.5)
call s:Expect('greeting = load(id) match', 'load', 'emelaFunctionCall')
call s:Expect('greeting = load(id) match', 'match', 'emelaConditional')
call s:Expect('Some(user) ->', 'Some', 'emelaPreludeConstructor')
call s:Expect('Some(user) ->', '->', 'emelaOperator')
call s:Expect('None -> "guest"', '"guest"', 'emelaString')
call s:Expect('first = users', '|>', 'emelaOperator')
call s:Expect('first = users', 'fn', 'emelaKeyword')
call s:Expect('first = users', 'match', 'emelaConditional')
call s:Expect('first = users', 'None', 'emelaPreludeConstructor')

" Declaration keywords and modifiers get the same color as `fn`.
function! s:ExpectLinked(group, target) abort
  let s:checked += 1
  " Read the direct link: under `-u NONE`, Keyword itself links on to Statement.
  let l:actual = matchstr(execute('highlight ' . a:group), 'links to \zs\w\+')
  if l:actual !=# a:target
    call add(s:failures, printf('%s: expected link to %s, got %s', a:group, a:target, l:actual))
  endif
endfunction

for s:group in ['emelaKeyword', 'emelaStructure', 'emelaEffectDecl', 'emelaStorageClass']
  call s:ExpectLinked(s:group, 'Keyword')
endfor

" ftplugin
function! s:ExpectEqual(what, actual, expected) abort
  let s:checked += 1
  if a:actual !=# a:expected
    call add(s:failures, printf('%s: expected "%s", got "%s"', a:what, a:expected, a:actual))
  endif
endfunction

call s:ExpectEqual('commentstring', &l:commentstring, '# %s')
call s:ExpectEqual('comments', &l:comments, ':##,:#')
call s:ExpectEqual('shiftwidth', &l:shiftwidth, 2)
call s:ExpectEqual('expandtab', &l:expandtab, 1)
call s:ExpectEqual('suffixesadd', &l:suffixesadd, '.emel')
call s:ExpectEqual('module Http.Client', EmelaModuleToPath('Http.Client'), 'http/client')
call s:ExpectEqual('module Data.Json.{', EmelaModuleToPath('Data.Json.{Json,'), 'data/json')
call s:ExpectEqual('module JsonCodec', EmelaModuleToPath('JsonCodec'), 'json_codec')
call s:ExpectEqual('module Http2Client', EmelaModuleToPath('Http2Client'), 'http2_client')

" plugin: capability values from the language server are underlined in Neovim, also after a
" color scheme loads. `-u NONE` skips plugin/, so load it here.
runtime plugin/emela.vim
function! s:Underlined() abort
  return execute('highlight @lsp.typemod.variable.capability.emela') =~# 'gui=underline'
endfunction
if has('nvim')
  call s:ExpectEqual('capability underline', s:Underlined(), 1)
  colorscheme default
  call s:ExpectEqual('capability underline after colorscheme', s:Underlined(), 1)
else
  call s:ExpectEqual('loaded in Vim', exists('g:loaded_emela'), 0)
endif

" A multiline string in an interpolation in a string, from the formatter's test data.
call s:Open(s:root . '/crates/emela-fmt/test_data/next_syntax.emel')
call s:Expect('interp = "外', '#{', 'emelaInterpDelim')
call s:Expect('interp = "外', 'g(', 'emelaFunctionCall')
call s:Expect('interp = "外', '"""', 'emelaString')
call s:Expect('補間の中の複数行', '補間', 'emelaString')
call s:Expect('""") } 外"', ')', 'emelaInterp')
call s:Expect('""") } 外"', '}', 'emelaInterpDelim')
call s:Expect('""") } 外"', '外', 'emelaString')
call s:Expect('fn(x) { x }', 'fn', 'emelaKeyword')
call s:Expect('mask = ', '0xFF_FF', 'emelaNumber')
call s:Expect('浅い字下げの文字列', '浅い', 'emelaString')
call s:Expect('} else { "" }', 'else', 'emelaConditional')

" ---------------------------------------------------------------------------
" The examples and the formatter's test data (most of it from the spec) are valid Emela, so
" nothing in them should be highlighted as an error.
let s:files = globpath(s:root, 'examples/**/*.emel', 0, 1)
      \ + globpath(s:root, 'crates/emela-fmt/test_data/*.emel', 0, 1)
for s:file in s:files
  call s:Open(s:file)
  call s:ExpectNoErrors()
endfor

" The spec lives outside the repository. When $EMELA_SPEC names it, check each ```emel block too.
if !empty($EMELA_SPEC) && filereadable($EMELA_SPEC)
  let s:block = []
  let s:in_block = 0
  let s:count = 0
  for s:line in readfile($EMELA_SPEC)
    if !s:in_block && s:line ==# '```emel'
      let s:in_block = 1
      let s:block = []
    elseif s:in_block && s:line ==# '```'
      let s:in_block = 0
      let s:count += 1
      enew!
      call setline(1, s:block)
      setfiletype emela
      execute 'file spec-block-' . s:count
      call s:ExpectNoErrors()
    elseif s:in_block
      call add(s:block, s:line)
    endif
  endfor
endif

" ---------------------------------------------------------------------------
let s:out = map(copy(s:failures), '"FAIL " . v:val')
call add(s:out, printf('%d checks, %d failures', s:checked, len(s:failures)))
call writefile(s:out, '/dev/stdout')
if empty(s:failures)
  qall!
else
  cquit!
endif
