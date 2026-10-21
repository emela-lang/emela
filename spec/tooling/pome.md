# Pome `[pome.overview]`

A Pome is the unit of distribution and dependency (`[module.pome]`). This appendix defines the formats of `Pome.toml` and `Pome.lock`, and the details of fetching and verifying dependencies. The identification of Pomes, how versions are selected, the contents and hash of a version, the proxy, the sumdb, the rules for fetching and the lock, how modules of dependencies are visible, and the diagnostics about dependencies are in `[module.pome]`. The `emela pome` commands are in `[cmd.pome]`. This appendix does not repeat what is written there.

## Pome.toml `[pome.manifest]`

An example of `Pome.toml` is in `[module.pome]`.

- The format is TOML.
- In `[pome]`, `name` is the source path (`[pome.source-path]`), `version` is a semver without `v`, and `emela` is the version of the compiler.
- The major of `[pome].version` MUST match the `/vN` of `[pome].name`. The major of each version in `[dependencies]` and `[dev-dependencies]` MUST also match the `/vN` of the source path of its key. A mismatch is an error in `Pome.toml`. A mismatched tag is not treated as a version of that Pome (`[pome.source-path]`), so the mismatch is made an error earlier, at the file level.
- The values in `[dependencies]` and `[dev-dependencies]` are either a version string or a table with `version` and `module`.
- A path in `[replace]` is written relative to the project. The dependencies of a replaced Pome are read from the `Pome.toml` in the replacement directory. If its `[pome].name` differs from the replaced source path, it is an error.

## Identification `[pome.source-path]`

The rules for source paths are in "Identity" of `[module.pome]`.

- For a github.com source path, when the segments after the repository are treated as a directory, the trailing `/vN` is excluded.
- A tag whose directory does not match is not treated as a version of that Pome.

## Versions and contents `[pome.version]`

The rules for selecting versions and for the contents of a version are in "Version selection" and "Version contents and hashes" of `[module.pome]`.

- The dependency graph is built by following the `[dependencies]` of the selected version of each Pome.
- If the commit of the tag does not contain the directory of a directory Pome, that version does not exist.

## Fetching `[pome.fetch]`

The fetch paths and environment variables are in "Fetching and the lock" of `[module.pome]`.

- An entry of `EMELA_POMEPROXY` that is neither a URL nor `direct` is an error.
- The layout of the cache is as follows. Files are written to a temporary location and then renamed into place, so partially written contents are never visible.

| Path (`<p>` is the escaped source path) | Contents |
|---|---|
| `<cache>/<p>@<version>/` | The contents of that version. Read-only, with files 0444 and directories 0555 |
| `<cache>/download/<p>/@v/<version>.toml` | The verified `Pome.toml` of that version |
| `<cache>/sumdb/<key name>/latest` | The largest checkpoint seen (signed) |

proxy:

- In proxy URLs, the version is not escaped.

direct:

- `direct` uses the user's git configuration (credentials, `url.<base>.insteadOf`) as is, and does not restrict the protocol. This makes it possible to fetch private repositories matched by `EMELA_POMEPRIVATE`.
- For a directory Pome, the tree of that directory in the commit of the tag is read.
- A version that contains a symbolic link or a submodule is treated as a version that does not exist (the same as a 410 from the proxy).
- The `@latest` of `direct` is the largest version that is not a prerelease (or, if none, the largest including prereleases). Because ingestion checks are not run locally, unlike the proxy, it does not skip versions that cannot be ingested.

## sumdb `[pome.sumdb]`

The sumdb records, checkpoints, and URL layout are in "sumdb" of `[module.pome]`.

- Signature verification is strict, and rejects non-canonical signatures and weak keys.
- The contents of a version are checked against the first line of the record. When only `Pome.toml` is fetched (when walking the dependency graph), it is checked against the second line of the record.
- That a record is included in the tree of a checkpoint, and that the tree of a previously seen checkpoint is a prefix of the tree of a new checkpoint, are both verified without receiving proofs from the sumdb: the tree root is rebuilt from tiles and checked to match the root of the signed checkpoint. The contents of tiles are trusted only once the rebuilt root matches the signed root.
- The value of `EMELA_POMESUMDB` lists a key and a URL separated by whitespace. The key name `sum.emela.ab2m.link` differs from the host name because the key was not replaced when the host moved. Replacing the key would make every compiler that embeds the old key fail verification.

## Pome.lock `[pome.lock]`

```toml
version = 1

[[pome]]
path = "github.com/emela-lang/quince"
version = "v0.3.2"
commit = "0123456789abcdef0123456789abcdef01234567"
hash = "h1:..."
manifest = "h1:..."
external = ["src/kv.emel"]

[[pome]]
path = "github.com/emela-lang/brix"
version = "v0.2.0"
commit = "..."
hash = "h1:..."
manifest = "h1:..."
dev = true
```

What the lock contains and how the lock is used are in "Fetching and the lock" of `[module.pome]`.

- The format is TOML: `version = 1` followed by a sequence of `[[pome]]` tables.
- Each table has `path` (the source path), `version`, `commit` (40 hexadecimal digits), `hash` (the `h1:` of the contents), and `manifest` (the `h1:` of `Pome.toml`). `external` and `dev` can be omitted.
- The lock contains the whole dependency graph (the list of versions chosen by version selection). Direct dependencies alone would not be enough to fetch transitive dependencies and verify their hashes.
- Tables are ordered by source path. Each source path appears only once (Pomes with different `/vN` have different source paths, so they can both appear).
- The files containing `@external` that are counted in `external` are the `.emel` files, excluding those under `test/`, that have a line which, with whitespace removed, starts with `@external` (comment lines starting with `#` are not counted). Even when a version in the lock is reused, the files are counted again from the contents.
- `dev = true` is set on a Pome that is not in the list of versions selected for the graph of `[dependencies]` alone, and is not reachable by following the requirements of the versions selected from `[dependencies]`. When a test dependency raises the version of a shared dependency and that version requires a new Pome, that Pome is also needed for normal builds, so it is not marked `dev`.
- When there is no lock, or when a dependency in `Pome.toml` is not in the lock, `check`, `build`, `run`, and `test` print one `error: ` line to standard error suggesting `emela pome install`, and end with 1.
- The language server looks only at the cache. If a dependency cannot be used, it reports E0236 on `Pome.toml`.

## emela pome `[pome.command]`

The `emela pome` commands are defined in `[cmd.pome]`.
