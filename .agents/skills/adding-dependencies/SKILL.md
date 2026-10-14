---
name: adding-dependencies
description: "Adding or updating crate dependencies in the Golem workspace. Use when adding a new Rust dependency, changing dependency versions, or configuring dependency features."
---

# Adding Dependencies

Dependencies of the **root Cargo workspace** are centrally managed. Versions and default features
are specified once in the root `Cargo.toml` under `[workspace.dependencies]`, and root-workspace
members reference them with `{ workspace = true }`.

This rule does not cross Cargo workspace boundaries. Independently built workspaces under `sdks/`,
`test-components/`, `plugins/`, and `dev-tools/` manage dependencies in their own workspace root.
Read that workspace's `Cargo.toml` and scoped `AGENTS.md`; do not add its dependency to the
repository root merely to centralize the version.

## Adding a New Dependency to the Root Workspace

### Step 1: Add to root workspace Cargo.toml

Add the dependency under `[workspace.dependencies]` in the root `Cargo.toml`, specifying the version and any default features:

```toml
# Simple version
my-crate = "1.2.3"

# With features
my-crate = { version = "1.2.3", features = ["feature1", "feature2"] }

# With default-features disabled
my-crate = { version = "1.2.3", default-features = false }
```

Keep entries **alphabetically sorted** within the section. Internal workspace crates are listed first (with `path`), followed by external dependencies.

### Step 2: Reference from workspace member

In the member crate's `Cargo.toml`, add the dependency using `workspace = true`:

```toml
[dependencies]
my-crate = { workspace = true }

# To add extra features beyond what the workspace specifies
my-crate = { workspace = true, features = ["extra-feature"] }

# To make it optional
my-crate = { workspace = true, optional = true }
```

**Never** specify a version directly in a root-workspace member crate's `Cargo.toml`. Always use
`{ workspace = true }`. In an independent workspace, centralize at that workspace's root when it
uses workspace dependencies; otherwise follow its existing manifest pattern.

The same pattern applies to `[dev-dependencies]` and `[build-dependencies]`.

### Step 3: Verify

```shell
cargo check -p <crate> --all-targets
cargo test -p <crate> --lib -- --report-time  # If library behavior is affected
```

Verify every crate where the dependency was added or whose features changed. Also check directly affected consumers when a dependency change alters a public type, feature unification, build script, proc macro, or runtime integration.

Use `cargo make build` only for dependency updates with broad workspace impact, such as a widely used version bump, a workspace-wide feature change, or a patched foundational dependency. A dependency added to one leaf crate does not require a full workspace build.

## Updating a Root-Workspace Dependency Version

Change the version only in the root `Cargo.toml` under `[workspace.dependencies]`. All root-workspace
members automatically pick up the new version. For an independent workspace, update its own source
of truth instead.

## Pinned and Patched Dependencies

Some dependencies use exact versions (`=x.y.z`) to ensure compatibility. Check the `[patch.crates-io]` section in the root `Cargo.toml` for git-overridden crates (e.g., `wasmtime`). When updating patched dependencies, both the version under `[workspace.dependencies]` and the corresponding `[patch.crates-io]` entry must be updated together.

## Checklist

1. The affected Cargo workspace boundary was identified
2. Root-workspace dependency version specified in root `Cargo.toml` under `[workspace.dependencies]`
3. Root-workspace member crate references it with `{ workspace = true }`
4. Independent workspace dependency stays in that workspace's own manifests
5. Entry is alphabetically sorted in the applicable workspace dependencies list
6. Every consuming crate checks/builds and its affected tests pass
7. Direct consumers or the full affected workspace were checked when dependency impact is broader
