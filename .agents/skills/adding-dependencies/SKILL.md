---
name: adding-dependencies
description: "Adding or updating crate dependencies in the Golem workspace. Use when adding a new Rust dependency, changing dependency versions, or configuring dependency features."
---

# Adding Dependencies

All crate dependencies in the Golem workspace are centrally managed. Versions and default features are specified **once** in the root `Cargo.toml` under `[workspace.dependencies]`, and workspace members reference them with `{ workspace = true }`.

## Adding a New Dependency

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

**Never** specify a version directly in a member crate's `Cargo.toml`. Always use `{ workspace = true }`.

The same pattern applies to `[dev-dependencies]` and `[build-dependencies]`.

### Step 3: Verify

```shell
cargo build -p <crate>   # Build the specific crate
cargo make build          # Full workspace build
```

## Updating a Dependency Version

Change the version **only** in the root `Cargo.toml` under `[workspace.dependencies]`. All workspace members automatically pick up the new version.

## Pinned and Patched Dependencies

Some dependencies use exact versions (`=x.y.z`) to ensure compatibility. Check the `[patch.crates-io]` section in the root `Cargo.toml` for git-overridden crates (e.g., `wasmtime`). When updating patched dependencies, both the version under `[workspace.dependencies]` and the corresponding `[patch.crates-io]` entry must be updated together.

## Checklist

1. Version specified in root `Cargo.toml` under `[workspace.dependencies]`
2. Member crate references it with `{ workspace = true }`
3. No version numbers in member crate `Cargo.toml` files
4. Entry is alphabetically sorted in the workspace dependencies list
5. `cargo make build` succeeds
