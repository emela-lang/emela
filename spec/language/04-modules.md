# 4. Modules and visibility `[module]`

One file is one module, and the module name is determined by the file's path. There is no syntax for declaring a module inside a file.

## 4.1 Module names `[module.name]`

- The path from the source root is converted to PascalCase segment by segment, and the segments are joined with `.`. `src/http/client.emel` becomes `Http.Client`.
- File and directory names are lowercase snake_case. `json_codec.emel` becomes `JsonCodec`.
- A circular reference between modules is a compile error.

Details of the conversion:

- The converted name MUST fit the lexical class of type names (2.3). A name that contains no lowercase letter, such as `a.emel` (`A`) or `x_y.emel` (`XY`), and a name that contains non-ASCII characters are compile errors.
- A digit right after `_` is kept as is. `http_2.emel` becomes `Http2` and duplicates `http2.emel`.
- Two files that map to the same module name are a compile error. Files that differ only in case (`Foo.emel` and `foo.emel`) are also treated as duplicates.
- The files inside a directory with an invalid name do not become modules.
- Files and directories that start with `.`, and files other than `.emel` files, are ignored.
- `prelude.emel` directly under the source root, files with the same name as a core module (`string.emel`, `list.emel`, and so on), `std.emel`, and `std/` are reserved names and cannot be placed there (E0203). The modules of the std layers are placed under `Std`, so adding a module to std does not add reserved names (15.2).
- A directory with an invalid name gets exactly one diagnostic, on that directory.
- Directories are traversed in name order so that the order of diagnostics is deterministic.
- For duplicate modules (E0204), the file that comes first in path order is kept, and the diagnostic is reported on the later file. For files that differ only in case, the one with the invalid name gets the duplicate diagnostic in addition to the naming diagnostic (E0201). This is because they are the same file on a case-insensitive file system.
- An import cycle (E0206) gets one diagnostic for each group of modules that import one another, showing the shortest cycle from the starting module back to itself. The starting module is the module of the group whose name comes first in code point order (`App.A` before `App.B`). The location is the import statement in the starting module that imports the next module in the cycle. When the group is not a simple cycle, not every module of the group is listed. A self-import is reported as a cycle of length 1, with a dedicated message.
- An import whose target module does not exist (E0205) is not counted when detecting cycles.

Projects and source roots:

- A directory that contains `Pome.toml` is a project. The source root is `<project>/src`. The contents of `Pome.toml` are defined in 4.7.
- The path given to the `emela` command is turned into an absolute path with symbolic links resolved, and its ancestors are searched upward. The directory of the first `Pome.toml` found becomes the project.
- If a directory is given and no `Pome.toml` is found, it is a compile error. The entry of a project is `<project>/src/main.emel`. `src/lib.emel` is the library root, and becomes the root module when the project is depended on (4.7). A project MAY have both. Without `main.emel`, `emela run` is not possible (E0230). In a library-only project, the entry for `check` and `test` is `lib.emel`, and `build` only checks.
- If a file is given and no `Pome.toml` is found, it is treated as a single file. That file is the entry, and the modules are only the `.emel` files directly in the same directory as the file (subdirectories and ancestors are not searched). Only the files reachable from the entry through imports are read and checked. Errors in unreachable sibling files do not appear in diagnostics.
- In single-file mode, naming errors and reserved-name diagnostics are reported only when the file is the entry, and duplicate-module diagnostics are reported only when that module name is reached through imports. Naming errors in unreachable sibling files are not reported. If the entry does not become a module, no other files are read (only E0210 and the naming errors are reported).
- When the entry file does not become a module because of a naming error, E0210 is reported in addition to the naming diagnostic.
- If a file is given and a `Pome.toml` is found, but the file is outside both the source root and the test root, it is a compile error (E0208).
- `<project>/test` is the test root, which holds modules used only by tests (13.1). Module names are determined by the path from the test root, using the same rules as the source root, and go into the same table as the modules of the source root. A module with the same name in both is a duplicate (E0204). Reserved names are the same, and in addition `main.emel` and `lib.emel` cannot be placed there (E0203). Single-file mode has no test root.
- The test root is read by `check`, `test`, and the language server, and not by `build`, `run`, or `doc`. When it is not read, the modules of the test root are not part of the normal build.
- The output directory is `<project>/target/emela/<target>` (`js`, `c`, `wasm`) for a project, and `target/emela/<target>` in the file's directory for a single file.
- Paths in diagnostics are relative to the project (for a single file, to the file's directory).

## 4.2 import `[module.import]`

```emel
import Http.Client # call as Client.get(...)
import Data.Json.{Json, decode} # use Json and decode unqualified
```

- `import A.B` brings the last name, B, into scope.
- Only the names listed in braces can be used unqualified. There is no notation that imports everything.
- Imports are placed at the top of the file. Comments do not count. An import after a declaration is a compile error (E0127).
- Aliases (`as`) are not part of the language yet.
- The diagnostics for an undefined module (E0205) and an import cycle (E0206) point to the path part (`A.B` in `import A.B.{x}`), not to the whole import statement.
- An import that cannot be read because of a syntax error (no path, an error inside the path, an unclosed `.{`, or leftover input right after the statement as in `import A.`) does not count as an import of a module. An import after a declaration (E0127) does count.

## 4.3 pub `[module.pub]`

- Declarations are private by default. Only declarations marked `pub` are visible from other modules.
- Only top-level declarations can be marked `pub`. The targets are `fn` `type` `enum` `error` `effect` `handler` `layer` `const` `trait`.
- There is no way to make individual fields or variants public or private. In the same way, the functions of a trait follow the visibility of the trait: the functions of a `pub` trait can be called from other modules, and those of a trait that is not `pub` cannot (E0214).

## 4.4 opaque `[module.opaque]`

`opaque` exposes only the type name and hides the contents.

| Declaration | Type name | Construction | Pattern matching | Field access |
|---|---|---|---|---|
| `pub type` / `pub enum` | Public | Public | Public | Public |
| `pub opaque type` / `pub opaque enum` | Public | Only in the defining module | Only in the defining module | Only in the defining module |

```emel
# src/email.emel
pub opaque type Email(value: String)

pub error InvalidEmail(input: String)

pub fn parse(input: String) -> Email fails InvalidEmail {
  if String.contains(input, "@") {
    Email(value: input)
  } else {
    fail InvalidEmail(input:)
  }
}

pub fn to_string(email: Email) -> String {
  email.value
}
```

From other modules, a value can be created only through `Email.parse(...)`. Having a value of type Email guarantees, through the type, that it passed validation.

## 4.5 Type annotations on public functions `[module.pub-signature]`

The return type, fails, and use annotations on a public function are optional (parameter types are required by 6.1). When they are omitted, the linter warns, and an LSP code action can insert the inferred result.

## 4.6 Name resolution `[module.resolution]`

The names in a module are split into three namespaces (types, constructors, and values) plus module names.

| Namespace | Contents |
|---|---|
| Types | type, enum, error, effect, handler, layer, trait |
| Constructors | type constructors, enum variants, errors, handlers with fields |
| Values | fn, const |

- Which namespace is looked up depends on where the name is written. On the left of `.`, only modules and Traits are looked up. In a type position, only types are looked up. For a type name in an expression or a pattern, only constructors are looked up. This is why the module `Email` and the type `Email` in 4.4 do not collide.
- A method call (`x.f(...)`, `[type.rules]`) looks up the functions of the module that defines the type of x without looking at the scope, so that module does not need to be imported. The functions of traits are looked up only in the traits that can be written unqualified in the calling module.
- `import Email` alone does not make the type `Email` usable unqualified. Write `Email.Email`, or import it with `import Email.{Email}`.
- A name on the left of `.` is looked up among the module's definitions and imports, and if not found, among the built-in modules and the Traits of the Prelude. If both a module and a Trait are found at the same level, it is a compile error (E0218).
- The B in `A.B.f` is limited to a pub Trait of module A. There are no nested modules. An effect cannot be written on the left of `.` (`Logger.info` is a compile error. 8.2).
- In the `import A.B.{x}` form, B is also brought into scope as a module. Each listed name imports the pub definitions with that name from every namespace that has it (for a type, the type and the constructor).
- Imported names are not re-exported. Other modules can look up only the names that a module defines. Importing the same definition twice is not a duplicate.
- Variants and type constructors follow the pub and opaque of their parent type. Variants can be imported individually by name.
- A duplicate in the same namespace is a compile error (E0213). Declaring the same name twice among effect operations, trait functions, fields, or a list of type parameters is the same compile error. Two enums in the same module that have variants with the same name are duplicates, and so are `type Green` and a variant `Green`.
- A module's definitions and imports MAY shadow a name with the same name in the Prelude. The compiler warns (W0202). This keeps existing code from breaking when names are added to the Prelude (the same idea as type parameters in 2.3).
- Built-in modules (String, List, and so on) can be written on the left of `.` without an import. When a name X in a type position is found nowhere, and a built-in module X has a type X with the same name, the name resolves to that type (`Option[List[User]]` can be written without an import). This rule does not apply to source modules.
- Immediate (9.5) and Async (8.7) are visible without an import.
- The modules of the bundled core go into the same table as the source modules and are always read, even in single-file mode. Being writable on the left of `.` without an import, and resolving a name in a type position to a core type, apply only to core modules whose name has a single segment.
- Binding the same name twice in one pattern or one parameter list is a compile error (E0220). In a binding, the right-hand side is resolved first, and then the names of the pattern are added.
- `self` is visible inside the operations and the release of a handler. It is not visible inside init (because init is the side that creates the handler value). A handler with fields has a constructor with the same name, and init can write `PostgresUsers(conn: ...)`.
- The top-level constructor pattern of an escape arm is limited to errors (E0217). `_` and variables pass name resolution but are compile errors in type checking (E0320, 7.3).
- If a name in an import list is not found, it is E0212. If it is found but none of the definitions are pub, it is E0214.
- Importing two different definitions with the same name is E0213.
- When `M.x` looks up a definition that is not pub, E0214 is reported, and checking continues as if the name resolved to that definition.
- Writing a module name in a type position is E0217, "a module, not a type" (for example, writing `fn f(e: Email)` with only `import Email`).
- Writing a handler, layer, or trait in a type position is E0217, and writing an effect there is E0221. A `fails` clause accepts only errors and type parameters. A `use` clause accepts only effects and type parameters. A `use X` expression accepts only effects. `with` and the contents of a layer accept only handlers or layers (and label items, `[effect.label]`). Constraints, derive, impl, and supertraits accept only traits. Anything else is E0217.
- Inside the modules of the bundled core, the warning for shadowing the Prelude (W0202) is not reported.

## 4.7 Pome `[module.pome]`

A Pome is the unit of distribution and dependency. A project with a `Pome.toml` (4.1) is one Pome. There is no central registry. The source of truth for a Pome's identity and versions is its Git repository and its tags. There are no accounts and no name registration. Dependencies are fetched through a proxy, and their contents are verified with a checksum DB (sumdb). The proxy holds only copies of Git, and the sumdb holds only records of the form "the contents of this version were these". Neither can create a Pome or version that does not exist in Git.

The formats of `Pome.toml` and `Pome.lock` and the details of fetching and verification are in `[pome.overview]`, and the `emela pome` commands are in `[cmd.pome]`.

```toml
# Pome.toml
[pome]
name = "github.com/emela-lang/quince"   # source path
version = "0.3.2"
emela = "0.20.1"                         # minimum compiler version required
module = "Quince"                        # root name when depended on (optional)

[dependencies]
"github.com/emela-lang/x" = "v0.2.0"                              # this version or later
"github.com/acme/http"    = { version = "v1.4.0", module = "AcmeHttp" }

[dev-dependencies]
"github.com/emela-lang/brix" = "v0.2.0"                           # used only by tests

[replace]
"github.com/emela-lang/x" = { path = "../x" }    # takes effect only when building this Pome

[build]
target = "c"                                     # default build target (js if omitted)
```

Identity:

- A Pome is identified by its source path (`host/path`, the form without `https://` and a trailing `.git`). The shorthand `github:owner/repo` is accepted only as command input. `Pome.toml` and `Pome.lock` always use the source path.
- In `[pome]`, `name`, `version`, and `emela` are required, and `module` and `targets` can be omitted.
- For a github.com source path, the part up to `github.com/<owner>/<repo>` is the repository, and the segments after it are a directory inside the repository. One repository can hold multiple Pomes (`github.com/emela-lang/x/platform-cloudflare` is the Pome in the directory `platform-cloudflare` of the repository `github.com/emela-lang/x`). On other hosts, the whole source path without `/vN` is the repository, and directory Pomes are not possible.
- The version of a directory Pome is expressed by a tag prefixed with the directory (`platform-cloudflare/v0.1.0`). Its contents are the files under that directory, and paths are relative to the directory. The tag of the Pome at the repository root stays `v0.1.0`.
- A Pome with major version 2 or higher appends `/vN` (N is the major version) to the end of its source path (`github.com/acme/http/v2`, `github.com/emela-lang/x/platform-cloudflare/v2`). The source path without `/vN` is the location of that Pome, and it is a different Pome from the v0 and v1 Pome at the same location. Versions of a source path without `/vN` are limited to v0 and v1, and versions of a source path with `/vN` are limited to major version N. A tag that does not match is not considered a version of that Pome (it does not appear in `@v/list` and cannot be ingested). Endings such as `/v0`, `/v1`, and `/v02` cannot be written.
- The `target` in `[build]` is the default build target for `check`, `build`, `run`, and `test` (`js`, `c`, `wasm`, chapter 16). If omitted, it is `js`. The `--target` option of a command takes precedence. Like `[replace]`, it takes effect only when building that Pome and is ignored when the Pome is depended on. An unknown target is a compile error (E0232).
- When `Pome.toml` cannot be read as TOML, or when the `target` in `[build]` is not a string, it is treated as if no `target` were specified.

Modules:

- The modules of a Pome that is depended on are visible under that Pome's root name. `src/lib.emel` is the root module (`Quince`), and `src/router.emel` becomes `Quince.Router`. `src/main.emel` is not used when the Pome is depended on.
- Inside a Pome that is depended on, the module of `src/lib.emel` can be referenced as `Lib`, and other modules by the names given by the rules of `[module.name]`. The depending side cannot reference `Quince.Lib` or `Quince.Main`.
- The default root name is the last segment of the source path converted to PascalCase by the rules in 4.1 (`-` is treated the same as `_`. `json-codec` becomes `JsonCodec`). A trailing `/vN` does not count as a segment (`github.com/acme/http/v2` becomes `Http`). The author can change it with `[pome].module`, and the depending side can rename it with `module` in a `[dependencies]` table. When depending on two major versions of the same Pome, the root names collide (E0204), so the depending side renames one of them.
- `Std`, the names of core modules (`List`, `String`, and so on), and `Prelude` cannot be used as root names. A collision with the depending Pome's own modules or with another dependency's root name is a compile error (E0204), and the depending side renames it.
- An error in a root name is reported at the location in the `Pome.toml` that decided that root name. A reserved name is E0203. A collision with the first segment of the Pome's own modules or with another dependency's root name is E0204. A name that does not fit the lexical class of type names (`[lex.name]`), and a source path from which no root name can be made, are E0235. Each of these comes with a note suggesting to give a different root name with `module`.
- Dependencies of dependencies (transitive dependencies) are not visible. A module can import only the Pomes listed in its own `[dependencies]`, and, from tests, the Pomes listed in `[dev-dependencies]` ("Test dependencies" below).
- Names in an import are looked up from the Pome that the importing module belongs to: first the Pome's own modules, then the root names of its own `[dependencies]`, then the core modules.
- All modules under a Pome's source root are visible to the depending side. Modules under the test root (4.1) are not.
- `@test` functions in dependencies are not collected, and the test roots of dependencies are not read. `emela doc` does not output the modules of dependencies.
- In the messages of diagnostics and defects, modules of dependencies appear under names that do not overlap within the dependency graph (`Quince.Router`). If the dependency graph has two Pomes with the same root name, one of them gets a name with a number appended (`Quince2.Router`).
- The paths in diagnostics for files of dependencies are absolute paths inside the cache (`[pome.fetch]`).
- `@external` in a Pome used as a dependency can be used only if the Pome being built allows it ("Pomes that touch the host" below).

Pomes that touch the host:

```toml
# the app's Pome.toml
[dependencies]
"github.com/emela-lang/x/platform-cloudflare" = "v0.1.0"

[external]
allow = ["github.com/emela-lang/x/platform-cloudflare"]

# Pome.toml of platform-cloudflare
[pome]
name = "github.com/emela-lang/x/platform-cloudflare"
version = "0.1.0"
emela = "0.21.0"
targets = ["js"]
```

- `@external` in a Pome in the dependency graph can be used only when that Pome is in the `allow` list (a list of source paths) in the `[external]` of the Pome being built. No version is written. Transitive dependencies also cannot use it unless written in `allow`. `@external` in a dependency that is not allowed is a compile error (E0233).
- `@export` counts as `@external` for `[external] allow`. An `@entry` that names an entry adapter of a Pome that is not allowed is E0233.
- E0233 is reported only on the first `@external` of each module of a Pome that is not allowed. This is because allowing the Pome makes all `@external` in that module usable at once.
- Like `[replace]`, `[external]` takes effect only when building that Pome and is ignored when the Pome is depended on. This ensures that dependencies that run host code always appear as a list in the app's `Pome.toml`.
- A source path in `allow` that is not in the dependency graph produces a warning (W0204).
- An `@external` fn in a dependency cannot be pub (E0237). The path to the host goes through that Pome's handlers. External functions with use omitted and external functions that are suspend fns can be used only inside handlers in the first place (12.2), so the only path to the host visible from the depending side is effects. An external function with an explicit `use {}` can be called from anywhere inside that Pome. An `@external` type MAY be pub (it is a value whose contents cannot be opened, and only passing it around does not touch the host).
- `targets` in `[pome]` declares the targets the Pome runs on (`js`, `wasm`, `c`). If omitted, it is considered to cover all targets. If the `targets` of a Pome in the dependency graph does not include the build target, it is a compile error (E0238). A Pome without `@external` does not need to write it. `check` and the LSP do not decide a target, so they do not perform this check. Even if the declaration and the actual `@external` bindings disagree, the check of reachable external functions in 12.1 (E0412) is still performed.
- E0238 is reported in the `Pome.toml` of the Pome that declared the targets. The note shows the Pomes that depend on that Pome (`this Pome` if the Pome being built depends on it directly).
- Target-specific parts are confined to effect handlers, and the entry decides which handlers to provide. For this reason, splitting into a Pome with only effects (usable on all targets), target-specific Pomes with handlers and `@external`, and per-target entry Pomes is recommended. There is no way to write per-target dependencies.

```
github.com/emela-lang/x/kv                    only effect Kv. no targets
github.com/emela-lang/x/platform-cloudflare   handler WorkerKv. targets = ["js"]
github.com/me/app                             logic. depends only on x/kv and is written with use { Kv }
github.com/me/app/worker                      JS entry. depends on app and platform-cloudflare, allows it in [external]
```

Test dependencies:

- `[dev-dependencies]` are dependencies used only by tests. They are written the same way as `[dependencies]`, and root names are decided and renamed the same way. Like `[replace]`, they take effect only when building that Pome and are ignored when the Pome is depended on. The `[dev-dependencies]` of dependency Pomes are not fetched.
- `[dev-dependencies]` are read only by the commands that read the test root (`check`, `test`, and the language server). `build` and `run` do not read them.
- The root names in `[dependencies]` and `[dev-dependencies]` are checked together. The same root name is E0204.
- Only modules of the test root (4.1) can import test dependencies. Importing one from a module of the source root is a compile error (E0239, 13.1).
- Writing the same Pome in both `[dependencies]` and `[dev-dependencies]` is a compile error (E0240). Tests can use the Pomes in `[dependencies]` as they are.
- Version selection (below) runs only once, on the graph that combines `[dependencies]` and `[dev-dependencies]`. If a test dependency raises the requirement for a shared dependency, the normal build also uses that version. This keeps the version tested the same as the version shipped. The `[dev-dependencies]` of test dependencies are not followed.
- The `allow` of `[external]` (E0233) and `[pome].targets` (E0238) apply to the test dependency graph as well. Both are checked only when the test dependency graph is read (`check`, `test`, and the language server).

Version selection:

- A version is expressed as a git tag of the repository (semver with a `v` prefix, as in `v0.3.2`). There is no build metadata (the part after `+`). The tag without the `v` MUST match `[pome].version` in the `Pome.toml` of that tag.
- Publishing is just pushing a tag. The proxy ingests a version the first time someone requests it. `emela pome publish` can make the proxy ingest it in advance and check the recorded contents (appendix B).
- A version in `[dependencies]` means "this version or later". There is no range syntax.
- The version used is the largest of the requirements for that Pome written in the dependency graph (Minimal Version Selection). It does not change when a new tag appears, unless someone raises a requirement. In one build, one Pome uses only one version.
- A major version of 2 or higher is a separate Pome with `/vN` (see Identity). An author who raises the major version adds `/vN` to `[pome].name` and tags. This lets two incompatible major versions coexist in one build. v0 and v1 are the same Pome, and compatibility between them is the author's responsibility.
- `emela` is the minimum compiler version required to build that Pome. A dependency that requires a version newer than the current compiler is a compile error. A dependency for an older minor version is built with a warning, and if a compile error occurs inside the dependency, the targeted version is shown in a note. Version selection does not look at `emela`. There is no mechanism for switching the meaning of versions per module (editions, 18.1 #12).
- `emela` versions are compared numerically in major.minor.patch order, and missing components are treated as 0. A version that is too new and a value that cannot be read as a version are compile errors (E0234). The warning for a dependency for an older minor version (the major and minor pair is smaller than the current compiler's) is W0203.

Version contents and hashes:

- The contents of a version are the files of the git tree that the tag's commit points to (for a directory Pome, the files under that directory). They are not a working tree, so `.gitattributes` conversions and `export-ignore` do not apply. Anyone fetching from anywhere gets the same contents.
- A version that contains a symbolic link or a submodule, a version without `Pome.toml`, and a version whose `[pome].name` differs from the source path cannot be ingested. The limits are 10,000 files, 32 MiB in total, and 64 KiB for `Pome.toml`. The executable bit is not part of the contents.
- The content hash `h1:` is computed as follows. For each file, build the line "hexadecimal sha256 of the contents, two spaces, the path, a line feed", sort the lines by the UTF-8 bytes of the path, and concatenate them into one string. Encode the sha256 of that string in base64 and prefix it with `h1:`. Paths are `/`-separated relative paths and do not include `.git`.
- The hash of `Pome.toml` is the `h1:` of a tree that contains only that `Pome.toml`. When walking the dependency graph, only the `Pome.toml` of each version is fetched, so it is verified with this hash separately from the whole contents.

proxy:

- The proxy is an HTTP server that serves versions of Pomes read-only. In URLs, each uppercase letter in the source path is replaced with `!` followed by the lowercase letter (`github.com/Acme/x` becomes `github.com/!acme/x`). This prevents collisions on case-insensitive storage.

| URL (`<p>` is the escaped source path) | Contents |
|---|---|
| `<p>/@v/list` | The list of semver tags, one per line |
| `<p>/@latest` | The `.info` of the largest non-prerelease version that can be ingested. If there is none, the largest including prereleases |
| `<p>/@v/<version>.info` | JSON `{"version", "commit", "time"}`. `time` is the commit time (RFC 3339, UTC) |
| `<p>/@v/<version>.toml` | The `Pome.toml` of that version |
| `<p>/@v/<version>.tar.gz` | The contents of that version. Paths are relative to the Pome's root |

- When asked for a version it does not have yet, the proxy fetches it from the repository, checks the conditions above, records the hash in the sumdb, and then serves it. A version that cannot be ingested gets 404 or 410, with the reason in the body.
- `@latest` tries the non-prerelease versions from largest to smallest, then the prerelease versions from largest to smallest, and skips versions that cannot be ingested (404, 410). This lets `emela pome add` add a dependency with the version omitted even when the latest tag is broken. It tries at most five versions, and if none can be ingested, it returns 404. A temporary fetch failure (502) is not skipped, and that failure is returned. Skipping it would mistake an older version for the latest.
- Once ingested, a version does not change even if the author moves or deletes the tag. Only `@v/list` reflects the current tags in Git (it MAY be cached for a short time).
- The proxy serves only what can be fetched from Git. There is no way to upload an archive by hand. Currently it serves only Pomes on github.com.

sumdb:

- The sumdb is a transparency log (a Merkle tree) that appends the following two lines for each ingested version.

```
github.com/emela-lang/quince v0.3.2 h1:...
github.com/emela-lang/quince v0.3.2/Pome.toml h1:...
```

- Records are append-only and are never rewritten or deleted. There is exactly one record per version, and the contents first ingested become the contents of that version. Contents after a tag is moved cannot be ingested as that version.
- The head of the tree (the checkpoint) is a signed note signed with the sumdb's Ed25519 key. The URL layout follows C2SP tlog-tiles, with `checkpoint`, `lookup/<p>@<version>`, `tile/<L>/<N>`, and `tile/entries/<N>`.
- Each time a command looks up a record, it verifies that the record is included in the tree of a signed checkpoint, and that the tree of a previously seen checkpoint is a prefix of the tree of the new checkpoint. Seen checkpoints are stored in `~/.emela/pomes/sumdb/`. If the sumdb shows different records to different users or deletes a record, this check detects it.

Fetching and the lock:

- The fetch route is set by the environment variable `EMELA_POMEPROXY`. It is a comma-separated list of proxy URLs and `direct`, tried in order. The default is `https://pomes.emela.dev,direct`. The command moves to the next entry only when a proxy answers that the version does not exist (404, 410), and stops on any other failure. Setting it to `off` fetches from nowhere.
- `direct` fetches with the machine's `git` (`git ls-remote --tags` for the tag list, and a shallow clone of the tag for fetching). The hash is computed locally.
- A fetched version is checked against the sumdb record regardless of the route. The sumdb is set by the environment variable `EMELA_POMESUMDB` (the default is `https://sum.emela.dev` with its public key embedded in the compiler. The key name stays `sum.emela.ab2m.link`). Setting it to `off` skips the check.
- When the environment variable `EMELA_POMEPRIVATE` is set to a comma-separated list of source path prefixes, matching Pomes are fetched with `direct` without using the proxy or the sumdb. Prefixes are compared segment by segment (`github.com/acme` matches `github.com/acme/x` but not `github.com/acmex`). This keeps the source paths of unpublished repositories from being sent to the proxy and the sumdb.
- Fetched versions are extracted to `~/.emela/pomes/<source path>@<version>/` and kept read-only. The source path in the cache is escaped by the same rule as in proxy URLs (each uppercase letter becomes `!` followed by the lowercase letter), so that it does not collide on case-insensitive file systems. The location can be changed with the environment variable `EMELA_POME_CACHE`.
- For each dependency, `Pome.lock` records the source path, version, commit, the `h1:` of the contents, the `h1:` of `Pome.toml`, and the list of files that contain `@external` (omitted if there are none). A Pome reachable only from the test dependency graph gets `dev = true`, and `build` and `run` do not fetch it. The `emela pome` commands write it, and it is not edited by hand. An application (a Pome with `main.emel`) has a lock, and a library MAY have one. The depending side does not use the lock of a dependency.
- The sumdb is checked when a version not yet in the lock is written to the lock (by the `emela pome` commands). If it does not match the sumdb record, the version is not written to the lock and the command fails with an error.
- `check`, `build`, `run`, and `test` fetch a dependency listed in the lock if it is not in the cache, and use it after verifying the hash in the lock. They do not rewrite the lock and do not query the sumdb. A dependency in `Pome.toml` but not in the lock is an error, and the message points to `emela pome install`. A hash mismatch is an error, as suspected tampering. The language server does not access the network and reports missing dependencies as diagnostics.
- `[replace]` uses a local directory as that dependency. It takes effect only when building that Pome and is ignored when the Pome is depended on. The hash of a replaced dependency is not written to the lock.

Not included (to be considered in a later version): a workspace that groups multiple Pomes (Bushel), a search index (Orchard), vendoring dependencies into the local tree, a way to point to an untagged commit or a branch, shorthands other than `github:`, and a marker that withdraws a published version (retract).
