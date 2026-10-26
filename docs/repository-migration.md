# qbx-lua repository migration

`Qbox-project/qbx-lua` contains the linter, language server, and their shared Rust crates.
The editor adapters remain in `Qbox-project/qbx-editor`. A change to shared analysis and LSP
behavior now belongs in one pull request, checked by the workspace CI on Windows and Linux.

The migration completed on **30 September 2026**. The
[workspace import](https://github.com/Qbox-project/qbx-lua/pull/8),
[shared release preparation](https://github.com/Qbox-project/qbx-lua/pull/9), and
[editor integration update](https://github.com/Qbox-project/qbx-editor/pull/3) are merged.
[Tooling v1.0.5](https://github.com/Qbox-project/qbx-lua/releases/tag/v1.0.5) is the first release
containing both binaries. [Editor v1.0.5](https://github.com/Qbox-project/qbx-editor/releases/tag/v1.0.5)
uses the combined workspace and is published to VS Code Marketplace and Open VSX.
The [old server repository](https://github.com/Qbox-project/qbx-lua-ls) is archived.

## What moved

| Component | Current home |
| --- | --- |
| Command-line linter and formatter | `qbx-lua/crates/qbx_lint` |
| Language server, its tests, scripts and protocol docs | `qbx-lua/crates/qbx_lua_ls` |
| Shared parser, formatter, analysis and FiveM data | Other crates in `qbx-lua/crates` |
| Editor adapters and interfaces | `Qbox-project/qbx-editor` |

Clone only `qbx-lua` to work on either Rust tool. Open linter, formatter, analysis and LSP
issues and pull requests in [qbx-lua](https://github.com/Qbox-project/qbx-lua/issues);
editor adapter and UI issues belong in [qbx-editor](https://github.com/Qbox-project/qbx-editor/issues).
See the [main README](../README.md) for installation and usage, the
[server guide](../crates/qbx_lua_ls/README.md) for LSP features and setup, and
[CONTRIBUTING.md](../CONTRIBUTING.md) for workspace checks.

## Preserved history

The language server was imported into `crates/qbx_lua_ls` with a full-history subtree merge,
without squashing or rewriting its commits. The import joins the two existing commit graphs:

- Linter source tip: `6cdc955`.
- Language-server source tip: `3957972`.

All 103 commits reachable from those tips retain their original IDs, authors, committers,
timestamps and messages. Workspace integration is recorded in subsequent commits. Historical
server commits still describe paths at their original repository root; use `git log --follow -m`
for the history of an individual moved file.

The migration was merged with **Create a merge commit**, retaining both original commit graphs
on `main`.

The original repositories both used `v1.0.x` tags. The linter's tags are unchanged, and the
server's original tag objects and targets are published under `lua-ls/v1.0.0` through
`lua-ls/v1.0.4`, avoiding collisions. To view a moved file's history:

```sh
git log --follow -m -- crates/qbx_lua_ls/src/server.rs
```

These names do not trigger the release workflow's `v*` tag filter. Old GitHub release pages
and any closed issues or pull requests remain available in the original server repository.

## Build and release compatibility

One root `Cargo.toml` controls versions and dependencies, with one `Cargo.lock` and `target/`
directory. The executables remain `qbx-lint` and `qbx-lua-ls`.

The release workflow builds each tool separately for the existing five platforms, using the
same archive names as before. CLI builds use `lint-release` with abort-on-panic and without
the server's documentation features. Server builds use `release` with panic unwinding,
preserving LSP request recovery. A new `vX.Y.Z` release contains both sets of archives.

The root `action.yml` is available as `Qbox-project/qbx-lua@<tag>`.
Existing linter tags and their release assets are unchanged. Existing server releases remain
downloadable from `Qbox-project/qbx-lua-ls`; future releases are published here.

## Download and editor compatibility

Use [qbx-lua releases](https://github.com/Qbox-project/qbx-lua/releases) for both tools from
`v1.0.5` onward. Select `qbx-lint-<target>` for command-line use or `qbx-lua-ls-<target>` for an
LSP client; each archive contains that tool and its license. Shared releases include checksums
for both sets of archives.

`qbx-editor` CI and development scripts now use the combined workspace. Its release builds use
the matching workspace tag, and its Zed adapter downloads server archives from `qbx-lua`.
An editor release must use `v1.0.5` or a later shared tooling tag; older linter tags predate
the server import. Historical standalone server releases through `v1.0.4` remain available
in the archived repository for existing editor versions and download links.

## Repository rename and GitHub Action

After combining the Rust projects, the repository was renamed from
`Qbox-project/qbx-lint` to **`Qbox-project/qbx-lua`** to represent the whole Qbox Lua suite.
The binaries, crate names, configuration file (`qbxlint.toml`), lint suppression comments
and release asset names remain unchanged.

GitHub redirects normal repository links and Git clone/fetch/push operations from the old name.
Update existing remotes with:

```sh
git remote set-url origin https://github.com/Qbox-project/qbx-lua.git
```

GitHub Actions does **not** follow repository-name redirects. Update workflows from
`uses: Qbox-project/qbx-lint@...` to:

```yaml
- uses: Qbox-project/qbx-lua@v1.1.0
  with:
    paths: .
```

Keep the old `qbx-lint` repository name unused so its normal redirects continue working.
See [GitHub's repository rename documentation](https://docs.github.com/en/repositories/creating-and-managing-repositories/renaming-a-repository).

For editor development, clone the workspace as `qbx-lua` beside `qbx-editor`. The editor's build
scripts, resource fixtures and development server lookup use that directory; published extensions
continue to bundle the same `qbx-lua-ls` executable. The Zed adapter downloads the server from
`Qbox-project/qbx-lua` releases.
