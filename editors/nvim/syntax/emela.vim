" Vim/Neovim syntax file for the Emela language (0.20).
" Language:  Emela
" Filenames: *.emel
"
" Follows the lexer in crates/emela-syntax/src/lexer*. The lexical class of a name is fixed by
" its spelling, so most of this file is regular expressions over spelling. Where a word's role
" depends on its position (contextual words, constructors, type parameters), the rules below are
" heuristics; see NOTES.md.

if exists("b:current_syntax")
  finish
endif

let s:cpo_save = &cpo
set cpo&vim

syntax iskeyword @,48-57,_
syntax case match

" ---------------------------------------------------------------------------
" Names (spec 2.3)
"
" Defined before the keywords and literals: for items starting at the same column, the one
" defined last wins, and keywords always win over matches.

" A name that fits no class (`Http_client`, `Max_SIZE`) is a lexical error.
syntax match emelaNameError "\<\u\w*\>"
" Type name: starts uppercase, letters and digits only, at least one lowercase.
syntax match emelaType "\<\u[A-Za-z0-9]*\l[A-Za-z0-9]*\>"
" Upper name: uppercase letters, digits and `_`. A const, or a type parameter.
syntax match emelaConstant "\<\u[A-Z0-9_]*\>"
" A single capital (optionally with digits) is almost always a type parameter (`A`, `T1`).
syntax match emelaTypeParam "\<\u\d*\>"
" A type name applied to `(` builds or matches a value: a constructor.
syntax match emelaConstructor "\<\u[A-Za-z0-9]*\l[A-Za-z0-9]*\ze("
" A lower name applied to `(` is a call.
syntax match emelaFunctionCall "\<[a-z_]\w*\ze("

" ---------------------------------------------------------------------------
" Declarations (spec 3.1, 17.3) and the names they introduce

syntax keyword emelaInclude      import nextgroup=emelaModulePath skipwhite
syntax keyword emelaStorageClass pub opaque suspend
syntax keyword emelaKeyword      fn nextgroup=emelaFuncDecl skipwhite
syntax keyword emelaKeyword      const nextgroup=emelaConstDecl skipwhite
syntax keyword emelaKeyword      impl
syntax keyword emelaStructure    type enum error trait nextgroup=emelaTypeDecl skipwhite
syntax keyword emelaEffectDecl   effect handler layer nextgroup=emelaTypeDecl skipwhite

syntax match emelaModulePath "\<\u[A-Za-z0-9]*\%(\.\u[A-Za-z0-9]*\)*" contained
syntax match emelaFuncDecl   "\<[a-z_]\w*\>" contained nextgroup=emelaTypeParams
syntax match emelaConstDecl  "\<\u[A-Z0-9_]*\>" contained
syntax match emelaTypeDecl   "\<\u\w*\>" contained nextgroup=emelaTypeParams

" `[A, B: Eq + Show]` right after a declared name: a parameter follows `[` or `,`; the names after
" `:` and `+` are bounds and keep their ordinary highlighting.
syntax region emelaTypeParams matchgroup=emelaDelimiter start="\[" end="\]" contained
      \ contains=emelaTypeParamDecl,emelaType,emelaConstant,emelaTypeParam,emelaOperator,emelaComment
syntax match emelaTypeParamDecl "\%([[,]\s*\)\@<=\<\u\w*\>" contained

" ---------------------------------------------------------------------------
" Expressions (spec 3.1)

syntax keyword emelaConditional if else match
syntax keyword emelaException   fail escape
syntax keyword emelaEffect      use with
syntax keyword emelaStatement   assert
syntax keyword emelaSelf        self Self

" `as` is reserved without syntax in 0.20, and the words of spec 3.3 have no syntax at all.
" Writing any of them is an error.
syntax keyword emelaReserved as var return while loop let mut in where do try catch throw async
      \ yield defer macro mod

" Contextual words (spec 3.2). They are keywords only in the position shown; elsewhere they are
" ordinary names. In each pattern, the word would be a syntax error as a name.
"   -> T fails E | F,  fails {}
syntax match emelaContextual "\<fails\ze\s\+[A-Z{]"
"   type/enum ... derive Eq, Show
syntax match emelaContextual "\<derive\ze\s\+\u"
"   init { ... } and release { ... } in a handler
syntax match emelaContextual "\<\%(init\|release\)\ze\s*{"

" Bool constructors (spec 3.4) and the empty tuple.
syntax keyword emelaBoolean True False
syntax keyword emelaPreludeConstructor Some None
syntax match emelaUnit "\w\@<!()"

" ---------------------------------------------------------------------------
" Annotations (spec 3.5)

syntax match emelaAnnotationOther "@\h\w*"
syntax match emelaAnnotation "@\%(external\|test\|intrinsic\|internal\|deprecated\)\>"

" ---------------------------------------------------------------------------
" Operators (spec 6.6, 17.5). Longer forms first: the first alternative that matches is taken.

syntax match emelaOperator "|>\|||\|->\|\.\.\|[=!<>]=\|&&\|[-+*/%!<>=|]"

" ---------------------------------------------------------------------------
" Numbers (spec 2.4, 17.2)

syntax match emelaNumber "\<\d[0-9_]*\>"
" A number followed directly by letters (`1e9` `3px` `0XFF` `1.0e`) is a lexical error (E0104).
syntax match emelaNumberError "\<\d[0-9_]*\%(\.\d[0-9_]*\)\=\a\w*"
syntax match emelaNumber "\<0x\x[0-9a-fA-F_]*\>"
syntax match emelaNumber "\<0b[01][01_]*\>"
syntax match emelaFloat  "\<\d[0-9_]*\.\d[0-9_]*\%([eE][+-]\=\d[0-9_]*\)\=\>"

" ---------------------------------------------------------------------------
" Strings (spec 2.4)

" Any other `\` is an error (E0106); the seven escapes and `\u{...}` are defined after it so they
" win at the same column.
syntax match emelaEscapeError "\\.\|\\$" contained
syntax match emelaEscape "\\[\"\\ntr#]" contained
syntax match emelaEscape "\\u{\x\{1,6}}" contained

" A string cannot cross a newline (E0101), so an unclosed one ends at the end of the line. The
" end is not searched inside an interpolation, which may span lines.
syntax region emelaString start=+"+ skip=+\\\\\|\\"+ end=+"\|$+
      \ contains=emelaEscape,emelaEscapeError,emelaInterp
" `"""` is defined after `"` so it wins at the same column. `""""` is `"""` then `"`.
syntax region emelaString start=+"""+ skip=+\\\\\|\\"+ end=+"""+
      \ contains=emelaEscape,emelaEscapeError,emelaInterp

" `#{ ... }` holds ordinary code. Braces inside it nest, so `#{ f({ x }) }` closes at the last
" `}`. A comment inside runs to the end of the line, `}` included, as in the lexer.
syntax region emelaInterp matchgroup=emelaInterpDelim start="#{" end="}" contained
      \ contains=@emelaCode,emelaInterpBrace
syntax region emelaInterpBrace start="{" end="}" contained transparent
      \ contains=@emelaCode,emelaInterpBrace

" ---------------------------------------------------------------------------
" Comments (spec 2.2). `#` outside a string always starts a comment.

syntax keyword emelaTodo TODO FIXME XXX NOTE contained
syntax match emelaComment "#.*$" contains=emelaTodo,@Spell
syntax match emelaDocComment "##.*$" contains=emelaTodo,@Spell

" Everything that may appear in code, for the inside of an interpolation.
syntax cluster emelaCode contains=emelaNameError,emelaType,emelaConstant,emelaTypeParam,
      \emelaConstructor,emelaFunctionCall,emelaInclude,emelaStorageClass,emelaKeyword,
      \emelaStructure,emelaEffectDecl,emelaConditional,emelaException,emelaEffect,emelaStatement,
      \emelaSelf,emelaReserved,emelaContextual,emelaBoolean,emelaPreludeConstructor,emelaUnit,
      \emelaOperator,emelaNumber,emelaNumberError,emelaFloat,emelaString,emelaComment,
      \emelaDocComment

" Multiline strings and interpolations can open far above; read from the start of the file.
syntax sync fromstart

" ---------------------------------------------------------------------------

highlight default link emelaNameError          Error
highlight default link emelaType               Type
highlight default link emelaConstant           Constant
highlight default link emelaTypeParam          Identifier
highlight default link emelaConstructor        Function
highlight default link emelaFunctionCall       Function

highlight default link emelaInclude            Include
" Declarations and modifiers share `fn`'s color: some color schemes leave Structure and
" StorageClass uncolored.
highlight default link emelaStorageClass       Keyword
highlight default link emelaKeyword            Keyword
highlight default link emelaStructure          Keyword
highlight default link emelaEffectDecl         Keyword
highlight default link emelaModulePath         Type
highlight default link emelaFuncDecl           Function
highlight default link emelaConstDecl          Constant
highlight default link emelaTypeDecl           Typedef
highlight default link emelaTypeParamDecl      emelaTypeParam
highlight default link emelaDelimiter          Delimiter

highlight default link emelaConditional        Conditional
highlight default link emelaException          Exception
highlight default link emelaEffect             Statement
highlight default link emelaStatement          Statement
highlight default link emelaSelf               Special
highlight default link emelaReserved           Error
highlight default link emelaContextual         Keyword

highlight default link emelaBoolean            Boolean
highlight default link emelaPreludeConstructor Constant
highlight default link emelaUnit               Constant

highlight default link emelaAnnotation         PreProc
highlight default link emelaAnnotationOther    PreProc

highlight default link emelaOperator           Operator

highlight default link emelaNumber             Number
highlight default link emelaFloat              Float
highlight default link emelaNumberError        Error

highlight default link emelaString             String
highlight default link emelaEscape             SpecialChar
highlight default link emelaEscapeError        Error
highlight default link emelaInterpDelim        Delimiter

highlight default link emelaComment            Comment
highlight default link emelaDocComment         SpecialComment
highlight default link emelaTodo               Todo

let b:current_syntax = "emela"

let &cpo = s:cpo_save
unlet s:cpo_save
