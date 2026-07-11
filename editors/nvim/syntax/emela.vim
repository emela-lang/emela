" Vim/Neovim syntax file for the Emela language.
" Language:  Emela
" Filenames: *.emel

if exists("b:current_syntax")
  finish
endif

" Strings, with the escapes the lexer accepts: \n \t \" \\
syntax match emelaStringEscape "\\[nt\"\\]" contained
syntax region emelaString start=+"+ skip=+\\"+ end=+"+ contains=emelaStringEscape

" Numbers: Float (has a fractional part) before Int.
syntax match emelaFloat "\<\d\+\.\d\+\>"
syntax match emelaNumber "\<\d\+\>"

" Keywords.
syntax keyword emelaKeyword     fn let enum trait impl import module extern intrinsic pub for uses
syntax keyword emelaConditional if else match
syntax keyword emelaException   try catch throw throws panic
syntax keyword emelaBoolean     true false

" Built-in primitive and standard generic types.
syntax keyword emelaType Int Float Bool String Char Unit Array Option

" Any other capitalised identifier is a user type, enum, or variant.
syntax match emelaType "\<\u\w*\>"

" A lowercase identifier immediately followed by `(` is a call/definition.
syntax match emelaFunction "\<[a-z_]\w*\>\ze\s*("

" Operators. Define single-char forms first so multi-char forms win on overlap.
syntax match emelaOperator "[+\-*/%<>=!]=\?"
syntax match emelaOperator "||"
syntax match emelaOperator "|>"
syntax match emelaOperator "->\|::\|++\|&&\|?"

" Comments run from `--` to end of line (there are no block comments). Defined
" last so that at the `--` position it wins over the single-char `-` operator.
syntax match emelaComment "--.*$" contains=@Spell

highlight default link emelaComment      Comment
highlight default link emelaString       String
highlight default link emelaStringEscape SpecialChar
highlight default link emelaNumber       Number
highlight default link emelaFloat        Float
highlight default link emelaKeyword      Keyword
highlight default link emelaConditional  Conditional
highlight default link emelaException    Exception
highlight default link emelaBoolean      Boolean
highlight default link emelaType         Type
highlight default link emelaFunction     Function
highlight default link emelaOperator     Operator

let b:current_syntax = "emela"
