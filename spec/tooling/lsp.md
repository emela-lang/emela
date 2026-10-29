# Language server `[lsp]`

This section defines the contents of the responses that `emela lsp` (`[cmd.lsp]`) returns over the Language Server Protocol (LSP). Positions and ranges are expressed by the rules of LSP (line and UTF-16 position).

## Supported requests and synchronization `[lsp.capabilities]`

- The server reads and writes JSON-RPC with `Content-Length` over standard input and output. On `exit` after `shutdown`, it ends with exit code 0.
- Text is synchronized in full (`TextDocumentSyncKind::FULL`). The server receives the open, change, save, and close notifications. It does not ask for the text in the save notification (`includeText: false`), but uses the text if it arrives.
- The server answers the following requests.
  - `textDocument/formatting` (`[lsp.formatting]`)
  - `textDocument/hover` (`[lsp.hover]`)
  - `textDocument/definition`, `textDocument/references`, `textDocument/documentHighlight`, `textDocument/typeDefinition`, `textDocument/implementation` (`[lsp.navigation]`)
  - `textDocument/documentSymbol`, `workspace/symbol` (`[lsp.symbol]`)
  - `textDocument/foldingRange`, `textDocument/selectionRange` (`[lsp.structure]`)
  - `textDocument/completion`, `textDocument/signatureHelp` (`[lsp.completion]`)
  - `textDocument/rename`, `textDocument/prepareRename` (`[lsp.rename]`)
  - `textDocument/codeAction` (`[lsp.code-action]`)
  - `textDocument/inlayHint` (`[lsp.inlay-hint]`)
  - `textDocument/semanticTokens/full`, `textDocument/semanticTokens/range` (`[lsp.semantic-tokens]`)
- The only completion trigger character is `.`. The signature help trigger characters are `(` and `,`.
- The server does not answer `completionItem/resolve`. Completion items carry their full contents in the first response.
- Documents with URIs that are not `file:` (`untitled:` and so on) are not handled. Unsaved buffers get no diagnostics.
- The path of a URI is expressed with `%` encoding. All characters other than alphanumerics and `-._~/` are encoded.

## What is checked `[lsp.project]`

- The unsaved contents of an open document are used for checking instead of the contents on disk. Files that are not open are read from disk.
- The project a document belongs to is determined from its path by the rules of `[module.name]`. For a file inside a project, the input is the project directory, as with `emela check <project>` (the entry is `src/main.emel`). If no `Pome.toml` is found, the file is in single-file mode, and that file is the entry. Every open single file is checked as an entry.
- In single-file mode, only the files reachable from the entry through imports are read (`[module.name]`).
- If a folder in `workspaceFolders` of initialize (or `rootUri` if absent) has a `Pome.toml`, the server checks that project and reports diagnostics even before any file is opened. It does nothing for folders without `Pome.toml`.
- The check that produces diagnostics is the same as `emela check`. Because no target is chosen, target-dependent checks are not run (`[ext.decl]`).
- When answering a request, the server checks again with the contents at that point. When changes arrive in succession, the server reads all arrived notifications before checking, and if more changes have arrived after the check, it checks again without sending that result.

## Diagnostics `[lsp.diagnostics]`

- On every open, change, save, and close, the server re-checks the whole project the document belongs to and sends the results with `textDocument/publishDiagnostics`.
- The `code` of a diagnostic is the string of the diagnostic code (`E0107` and so on), and `source` is `emela`. Each note is appended to the end of the message as `\nnote: …`. Labels go into `relatedInformation`.
- `version` is set to the last received version for an open document, and is not set for files that are not open.
- A diagnostic without a determined position is attached at the start (0:0) of the following location.
  - Project resolution errors (E0208 pointing outside the source root and so on): the start of the opened document.
  - Diagnostics attached to a whole file, and diagnostics attached to a path not in the module table (a misspelled directory name and so on): the start of that path. Even if the path is a directory, it is sent with that URI.
  - Diagnostics not attached to any path (internal errors of the implementation and so on): the start of the entry. If there is no entry, the start of `Pome.toml`.
- Diagnostics for an open document are sent with the URI received from the editor. A file that is not open, if it is under a workspace folder, is sent with a URI formed by joining its path to the folder path received from the editor. Sending with a path that has symbolic links resolved makes the editor treat it as a different file.
- When two single files in the same directory are open and the same diagnostic appears twice, the duplicates are merged into one per file.
- On every check, all diagnostics are re-sent for every file that has diagnostics (even if unchanged from the previous time). An empty array is sent for files that were sent diagnostics last time and have none this time.
- Whether a file was sent diagnostics last time is compared by file path. When the way the URI of the same file is written changes (it was opened and became the editor's URI, or it was closed and went back to the server's URI), an empty array is sent to the old URI. The empty array is sent before the new diagnostics. This keeps the new diagnostics even if the editor treats the two spellings as the same file.

## Formatting `[lsp.formatting]`

- `textDocument/formatting` formats the whole document with the formatter (`[fmt]`). An open document is formatted from its unsaved contents, and a document that is not open from its contents on disk.
- The server returns a single `TextEdit` that replaces the whole document. If the document is already formatted, it returns an empty array.
- For a document that cannot be formatted (a syntax error, a failed formatter verification (`[fmt.io]`), an unreadable file, a URI that is not `file:`), it returns `null`. It does not respond with an error. Syntax errors are already reported as diagnostics (`[lsp.diagnostics]`).
- The formatting `options` (`tabSize`, `insertSpaces`) are ignored. Indentation and width are as in `[fmt.io]`.

## Hover `[lsp.hover]`

- For the name or expression under the cursor, the server shows the type and E (the error set) (`[type.inference]`). The display form of the type and E follows `[type.inference]`.
- The contents are Markdown, with the following written in an `emela` code block.
  - The name of a definition, and a name that refers to a definition: `name: type`. The type is the declared type, and for an inferred one, the generalized type.
  - A local variable (the bound position and references): `name: type`.
  - Any other expression: its type, and `fails E` on the next line if E is not empty.
  - For an expression, if R is not empty, a `use R` line also follows the `fails E` line.
  - The name of a handler: the form `handler H(conn: Connection): E`. A handler without fields is written without parentheses. A handler for an effect with type arguments is shown with the type arguments substituted (`handler Fixed: State[Int]`). If the E of init is not empty, an `init fails E` line is added, and if the requirements of the handler are not empty, a `use R` line is added.
  - The name of a layer: `layer L { H1, H2 }` (in initialization order). If the layer has requirements not satisfied inside it, a `use R` line is added.
  - The name of a label (`[effect.label]`), and a name that refers to a label (`use { AuditLog }`, `with AuditLog by H`): the declaration as is (`effect DebugLog of Console default`). Whether it falls back to the underlying effect can be read from `default` in the declaration.
  - A suspend fn (an effect operation and `@external suspend fn`): `suspend` is put before the type.
  - The type of a capability value is shown as `use Logger` (`log: use Logger`). For an effect with type arguments, `use State[Int]`.
  - The type of a handler value shows only the handler's name (`self: PostgresUsers`).
- Among the things containing the cursor position, the innermost (the one with the shortest range) is chosen. The end position of a range is not included.
- For a document that cannot be type checked, the server returns `null`.
- If the definition has a `##` documentation comment (`[lex.comment]`), it is shown as Markdown after the code block, separated by a horizontal rule (`[lsp.doc]`).
  - It is shown whether the definition is public or not.
  - For types and traits, and for methods that the type checker has not yet given a type, the code block shows the form of the declaration.
  - For a name implemented inside an impl or handler, the doc written on the implementation is shown, not the one on the declaration side.
  - The docs of an unnamed impl and of the init and release of a handler are shown only over their heads. They are not shown for expressions or bindings in the body.

## Navigation and references `[lsp.navigation]`

- `textDocument/definition` returns one location (`Location`) of the definition of the name. If there is no destination, it returns `null`. Definitions in the bundled core (`List.map` and so on) are treated as having no destination, and the server returns `null`.
- `textDocument/references` returns references inside the checked project (in single-file mode, the files reachable from the entry through imports). References inside the bundled core are not returned. If `includeDeclaration` is set, the name of the definition is also included. If there are no references, it returns `null`.
- `textDocument/documentHighlight` returns the occurrences of the same name in the same file. The name of the definition is included. Variables are immutable (`[fn.block]`), so the kind is always Text. If there is no name under the cursor, it returns an empty array.
- `textDocument/typeDefinition`, for a variable or expression, goes to the definition of the outermost named type of its type. It does not follow type arguments. For a capability value, it goes to the definition of its effect. For the name of a type or Trait itself, it goes to that definition. If there is no destination, as for built-in types, structural types, undetermined types, and types in the bundled core and std, it returns `null`. For expressions inside the body of a Trait impl, the destination is not derived from the type.
- `textDocument/implementation` returns, for the name of a Trait, the whole of each explicitly written impl, and for a method of a Trait, the names of the corresponding methods in the impls. For an impl that omits a default method, it points to the whole impl block. derive and built-in implementations are not returned. If there is nothing applicable, it returns an empty array.
- documentHighlight, typeDefinition, and implementation answer from the remaining results of name resolution and type checking even if the document has errors.
- The positions searched for a name include the end position of the range (the cursor right after the name). Things whose range contains the position are chosen first.
- Go to definition and find references answer per lexical token of a resolved name. A path like `Http.Client.get` is a separate reference at each segment.
- In an import path, each segment points to the module with the name up to that segment, if it exists (the `Util` of `import Util.Log` points to nothing if that module does not exist). The names in `{x, Y}` point to all of the imported definitions (for a type, the type and the constructor).
- The name of a handler operation and the name of a function in an impl point to the operation of the implemented effect and the function of the Trait.
- A method call is a reference of the function it calls. Go to definition, references and rename treat `x.f(...)` as `M.f(x, ...)`.
- The location of a module's definition is the start of its file (an empty range).
- When finding references, definitions with the same name position (a type and its constructor, an error and its constructor) are treated as the same. The results are returned in order of file and position. References to a module do not include the location of the definition.

## Symbols `[lsp.symbol]`

- `textDocument/documentSymbol` returns the declarations in the file as nested `DocumentSymbol`s. It does not check, and builds the result only from the syntax, so it answers even if there are type or import errors. It uses the unsaved contents if the document is open, and the contents on disk otherwise. For an unreadable file, it returns `null`.
- The kinds of symbols are as follows.

| Declaration | Kind |
|---|---|
| fn | Function |
| type, error | Struct |
| enum | Enum |
| variant | EnumMember |
| const | Constant |
| effect, trait | Interface |
| effect operation, fn of trait, impl, and handler | Method |
| handler | Class |
| layer, impl | Namespace |
| field | Field |

- The children in documentSymbol are the variants of an enum, the fields of a type, error, variant, and handler, the operations of an effect, and the fns of a trait, impl, and handler. Unnamed declarations are skipped. An impl is named `impl User: Show`, and its selection range is the name of the Trait.
- `workspace/symbol` returns the declarations of the projects in `workspaceFolders` (or `rootUri` if absent) and of the projects of open documents. It is the flattened form of the nesting of documentSymbol.
  - Modules that are not open are included. Dependency Pomes and the bundled core and std are not included. In single-file mode, only what is reachable from the entry through imports is included.
  - `query` filters by case-insensitive substring match on the name. If it is empty, everything is returned.
  - Only one symbol is returned per position. `containerName` is the name of the parent declaration or the name of the module.

## Folding and selection ranges `[lsp.structure]`

- `textDocument/foldingRange` and `textDocument/selectionRange` are built from the syntax tree of the unsaved contents without checking. They answer even if there are syntax, import, or type errors. For an unreadable document, they return `null`.
- foldingRange returns the following. Ranges of only one line are not returned. The end of a range is right after the closing bracket.
  - Kind region: multiline blocks, arms of match and escape, variants of an enum, fields of a type, error, and handler, and the bodies of trait, impl, effect, handler, and layer.
  - Kind imports: runs of imports on adjacent lines.
  - Kind comment: runs of comments on adjacent lines. `#` and `##` are not distinguished.
  - Runs of imports and comments are split by a blank line or by code in between.
- selectionRange returns, for each requested position, a chain of ranges from the token up through the parent syntax elements to the whole file. The same range is not repeated. If the position is on a token boundary, the token on the right is chosen. However, if only one side is whitespace or a comment, the other side is chosen. For a request that includes a position outside the document, it returns `null`.

## Completion and signature help `[lsp.completion]`

- `textDocument/completion` returns all candidates for names that can be written at that position. Filtering by the partially typed name is left to the editor. The order of candidates is kept by putting sequential numbers in `sortText`.
- The kinds of candidates are as follows.

| Candidate | Kind |
|---|---|
| local variable | Variable |
| fn | Function |
| const | Constant |
| constructor | Constructor |
| type, error | Struct |
| enum | Enum |
| effect, trait | Interface |
| handler, layer | Class |
| effect operation, trait function | Method |
| field | Field |
| module | Module |
| reserved word | Keyword |
| label of a named argument | Property (`insertText` is `x: `) |

- The candidates returned depend on the context determined from the text before the cursor, as follows.
  - Before `{` on an `import` line: the next segment after the module name (`Log` for `import Util.`). Names of functions and types, and the module itself, are not shown.
  - Inside `import A.{`: the pub definitions of module A (including enum variants).
  - After `.`, when the left side is a sequence of capitalized names: the name to the left of `.` is looked up in the same order as `[module.resolution]` (imported modules, core modules, Traits). For a module, its pub definitions. For a Trait, its functions.
  - After `.`, otherwise: if the type of the left expression is a capability value, the operations of the effect. If its head is a named type, its fields (the description is the text of the written type), then the functions that can be called as methods (`[type.rules]`; kind Method, the first parameter left out of the signature). For other types, empty. Even for a `.` at the end of a line, the type of the left expression is computed and candidates are shown (`l.from.`, `c.now().`).
  - Right after `use`: effects only. Right after `with`: handlers and layers only.
  - Otherwise: in this order, the visible local variables (innermost first), the definitions of this module (including variants), the names listed in imports, the imported modules, the core modules whose name has one segment (except `Prelude`), the definitions of Prelude, and reserved words (except `as`). For the same name, only the first one is shown.
- The type of an operation candidate of a capability value is the type with the effect's type parameters replaced by the type arguments of the capability value.
- At the start of an argument in a call (after `f(` or after a comma, when only a partially typed name is there), the labels of the callee not yet written are shown first (the name is `x:`, and the description is the argument's type), followed by value candidates. They are not shown after a label (the value position of `x: `).
- `textDocument/signatureHelp` returns exactly one signature: the type of the call and the labels of its arguments (there is no overloading). The range of an argument is expressed as UTF-16 positions inside the label of `ParameterInformation`. If the argument position of the cursor exceeds the number of arguments, `activeParameter` is not set.
- Signature help has the form `name(label: type, …) -> return fails E use R`, and is shown for the innermost call whose parentheses contain the cursor. For an unclosed call, the end position of the range is also treated as inside the parentheses. The argument at the cursor is the argument with that label if the partially typed argument starts with `label:`, and otherwise is determined by the number of commas outside nested brackets (commas inside strings are not counted). If the label of the partially typed argument is not among the remaining arguments, no argument is marked as active.
- For a call on the right side of a pipe (`x |> f(a)`), both signature help and label completion exclude the first argument, which the left side fills (`[fn.pipe]`).
- For a call on the right side of a pipe where the first argument is written by name (`xs |> List.fold(xs: ys)`), no argument is marked as active. When the right side is not a call (`x |> log.info`), no signature help is shown.
- The `documentation` of completion and signature help shows the doc of the definition (`[lsp.doc]`) as Markdown `MarkupContent`. The doc of a function is not split per argument. Completion of a named argument of a constructor shows the doc of the corresponding field.

## Rename `[lsp.rename]`

- `textDocument/rename` returns a `WorkspaceEdit` that renames the name across the whole project, using `changes` (edits per URI). Edits to files that are not open are included.
- A name that cannot be renamed, and a new name whose lexical class (`[lex.name]`) does not match, are answered with a `RequestFailed` error response. The messages have the forms `this name cannot be renamed` and `` `figure` is not a valid type name ``, and the editor displays them.
- `textDocument/prepareRename` returns `null` for a name that cannot be renamed.
- The names that cannot be renamed are modules, `self`, definitions in the bundled core and definitions without source, and impl. The new name MUST be a single lexical token of the same lexical class (`[lex.name]`) as the original name. Conflicts with names in the same scope are not checked.
- A type and its constructor, an effect operation and its handler implementations, and the `{x}` of an import are renamed together.
- The shorthand of a named argument (`NotFound(id:)`) and of a field pattern (`User(name:)`) is replaced with `id: new_name`, keeping the label.

## Code actions `[lsp.code-action]`

- There are two kinds: `refactor.rewrite` and `quickfix`. If `context.only` is given, the kinds are narrowed. The edit is a `WorkspaceEdit.changes`, and edits only the target file.
- `refactor.rewrite`: inserts the inferred result into the omitted annotations of a top-level fn (`[module.pub-signature]`). It is not tied to a diagnostic.
  - It is offered for a top-level fn with a body when the range covers the fn's header (from `fn` to just before the `{` of the body). It is also offered for fns that are not pub.
  - If the return type is omitted, `-> T` is added, together with `fails E` if the inferred E is not empty. Nothing is added to a fn that writes the return type and omits fails (because E is fixed to the written fails clause, `[error.fails]`).
  - If the `use` clause is omitted, `use R` is added if the inferred R is not empty. It goes after the fails clause or the return type.
  - Hidden sets of function types in arguments (`E(f)`) are excluded from E and R. When a hidden set appears in the return type, the return type and fails are not added.
  - A clause in which a definition appears that cannot be written unqualified in this file (types and errors of other modules not listed in an import) is not added.
  - The title has the form ``Add inferred `-> Int fails NotFound` ``.
- `quickfix` fixes a diagnostic, and lists the target diagnostic in `diagnostics`.
  - The target diagnostic is chosen from the result of checking at the time of the request, by diagnostic code, file, and range of the primary location. It does not depend on `context.diagnostics`. Diagnostics without a code are not handled.
  - If the requested range is empty, the diagnostics that contain the cursor (excluding the end of the range) are chosen. If it is not empty, the diagnostics whose ranges overlap are chosen.
- The fixes are as follows.
  - E0303 (match is not exhaustive): adds the examples of missing patterns reported by the exhaustiveness check (`[fn.match]`) at the end of the arms. The body of each arm is the Prelude `todo()`. Constructors are qualified with an already imported module if needed. When a name cannot be written in a single way, `_ -> todo()` is added. When the number of examples reaches the limit, the same `_ -> todo()` is added at the end.
  - E0325 (an impl is missing methods): adds the missing methods together at the end of the impl body, in declaration order. Only the argument names are taken from the trait, and types, fails, and use are not written. The body is `todo()`. It is offered only when the cursor is inside the head of the impl (`impl User: Show`), not at `{` or inside the body.
  - Imports for E0211 (undefined name): candidates are chosen from the module table and the checked public definitions. For a qualified name like `Io.print`, `import Io` is added. For an effect that is not imported, and the like, an import of the form `import Std.Console.{Console}` is added. Each candidate is a separate action. No edit is offered that makes an effect operation callable in the form `Effect.op` (`[module.resolution]`, `[effect.use]`).
  - Misspellings for E0211 and E0212: from the completion candidates at that position, excluding reserved words, labels, and names of a different uppercase/lowercase lexical class, the name is replaced with the closest name within the same edit distance as name resolution (within one third of the name length, at least 1). On a tie, the name from the inner scope is chosen.
  - E0318 and E0402 (the E and R of the body are not included in the declaration): writes the union of the body's E and R and the written clauses, by the same rules as inserting omitted annotations. Written elements are not removed. For E0402 on main, test, const, and handler operations, no edit that propagates use is offered.
  - E0144 and E0145 (the old form of impl and handler heads): for E0144, the head (from `impl` to the end of the head) is replaced with the rewritten head. For E0145, the text from after the name (or after the field sequence, if any) up to `implements` is replaced with `:`. The rewritten form is the same as the form shown in the diagnostic message (`[diag.list]`).
- There is no diagnostic for unused imports, so there is no quickfix that removes them.

## Inlay hints `[lsp.inlay-hint]`

- `textDocument/inlayHint` shows, inline, the types and the E and R (`[type.inference]`) that are not written in the source.
- The kind is `Type` for types and omitted annotations, and `Parameter` for the E and R of calls.
- A type (`: Int`) is shown right after the name. Others are shown with a space on the left.
- The following are shown.
  - Omitted annotations of a top-level fn: if the return type is omitted, `-> T fails E use R` (E and R only if not empty). If only the use clause is omitted, `use R`. Hidden sets (`E(f)`) are shown as is. Whether they can be written is not considered.
  - The type `: T` of bindings without type annotations (whose pattern is a single variable), of arguments of anonymous functions, and of arguments of handler operations and impl functions. Capability values, and types containing undetermined or broken types, are not shown.
  - The E and R (`fails E use R`) of calls and pipelines in statement position (a statement, the right side of a binding, the end of a block). Nothing is shown if both are empty. Nothing is shown for nested calls.
- If the end of the requested range is outside the file, it is treated as the end of the file.

## semantic tokens `[lsp.semantic-tokens]`

- Only capability values (`x = use X`, `[effect.use]`) are marked. Both the bound name and the names that refer to it (including inside closures) are marked. Other tokens such as functions and types are not returned. This avoids overlapping with the editor's regex-based highlighting.
- In addition to local variables that became capability values in type checking, a binding whose right side is `use X` (whose pattern is a single variable) is treated as a capability value even if `X` cannot be resolved. Even with syntax or type errors, the server returns as much as it can determine.
- The legend has one token type, `variable`, and one modifier, the custom `capability`.
- The server answers `full` and `range`, and does not answer `full/delta`. `resultId` is not set.
- If the end of the requested range is outside the file, it is treated as the end of the file. For a document that cannot be checked, the server returns `null`.

## Showing documentation comments `[lsp.doc]`

- The doc shown in hover, completion, and signature help is taken from the `##` comments of the sources used for checking (including unsaved contents, dependency Pomes, and the bundled core and std). Unlike `emela doc`, which shows only `pub` definitions, the language server shows docs whether the definition is public or not.
- From each line of a doc, only `##` and the one space right after it are removed. Line breaks and deeper indentation are kept.
- A doc that is empty or only whitespace is not attached. Candidates without a doc, local variables, and reserved words are not shown the doc of another definition.
