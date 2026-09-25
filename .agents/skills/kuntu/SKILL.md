---
name: kuntu
description: Work with the Kuntu Rust engine — kuntu-scan (disk usage scanning, cleanup candidates, dirty git repo discovery, iCloud eviction), kuntu-index search, and the workspace's other crates. Use when changing the engine, adding scan/cleanup features, understanding how kuntu is vendored into space-lens, or publishing kuntu-scan to crates.io.
---

# Kuntu

A local-first Rust file & storage engine. Embeddable crates, no transport, no
product. The flagship crate is **`kuntu-scan`** — the scanner and cleanup
engine behind [space-lens](https://github.com/HuakunShen/space-lens), which
vendors this repo as the `vendors/kuntu` git submodule and aliases the package
back to `space_lens`.

## Crates

| Crate | Purpose |
| --- | --- |
| `kuntu-scan` | Disk usage tree (`scan_directory`), cleanup candidates + removal plans (`find_candidates`, `build_removal_plan`, `execute_removal_plan`), dirty git repos (`find_dirty_git_repos`), snapshot provenance types, iCloud eviction planning (`cloud`, macOS only). Published to crates.io. |
| `kuntu-core` / `kuntu-crawler` / `kuntu-index` | File search: config, crawling, SQLite index + query. |
| `kuntu-daemon` / `kuntu-watcher` | Background index daemon and FS watching. |
| `kuntu-napi` | Node-API bindings for the **file search** only. |
| `kuntu-cli` | CLI adapter for the search/index side. |
| `kuntu-provider-spotlight` | macOS Spotlight provider. |

## Workspace conventions

- `#![deny(clippy::all)]` and `unsafe_code = "forbid"` — keep clippy clean.
- 2-space indent (`.editorconfig`/rustfmt), serde camelCase on public DTOs in
  `kuntu-scan`'s snapshot module, kebab-case in some enums; check the type.
- Engine options that gate traversal (e.g. `follow_symlinks`) are serde
  `#[serde(default)]` so older serialized inputs keep deserializing.
- Tests live inline (`#[cfg(test)]`) and build fixtures under
  `std::env::temp_dir()` with unique names — parallel tests must not share
  paths, and symlink fixtures must point **outside** the scan root (inode
  guards make in-root links order-dependent).

## Adding a feature across the stack

A scan/cleanup change is only user-visible after touching the consumers:

1. `kuntu-scan` here (engine + tests + clippy).
2. space-lens `packages/node/src/lib.rs` (+ `index.d.ts`) — the napi bindings
   for scan/cleanup live there, NOT in `kuntu-napi`.
3. space-lens `apps/cli` (clap flags/subcommand).
4. Bump the submodule pin in space-lens (`git add vendors/kuntu`).

## Releasing kuntu-scan to crates.io

The crate manifest is self-contained (no workspace inheritance) and publish
ready. From this repo:

```bash
cargo publish --dry-run -p kuntu-scan
cargo publish -p kuntu-scan
```

Bump `crates/kuntu-scan/Cargo.toml` version first (minor for features).
space-lens's CLI depends on `kuntu-scan = { version = "0.2", path = ... }`, so
publishing a compatible version keeps local path builds and crates.io builds
identical.

## Symlink policy (important)

Traversal uses `symlink_metadata` and **never follows symlinks unless the
caller sets `follow_symlinks: true`** (`ScanOptions`, `CandidateOptions`,
`DirtyGitRepoOptions`). When following, an inode-seen guard cuts cycles and
prevents double counting. Keep new traversal code on `symlink_metadata` +
`effective_metadata` + `mark_dir_visited` — do not reintroduce bare
`fs::metadata` walks.
