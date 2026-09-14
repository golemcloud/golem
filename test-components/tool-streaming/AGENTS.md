# Tool streaming integration components

This application owns the real provider/caller components used by the GOL-35 integration suite.

Build and copy the release artifacts from this directory with:

```shell
GOLEM_CLI="$(cargo metadata --no-deps --format-version 1 --manifest-path ../../Cargo.toml | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug/golem-cli"
GOLEM_RUST_PATH="$(cd ../../sdks/rust/golem-rust && pwd)" "$GOLEM_CLI" --preset release build --yes --skip-check
GOLEM_RUST_PATH="$(cd ../../sdks/rust/golem-rust && pwd)" "$GOLEM_CLI" --preset release exec copy
```

Always use the Golem CLI rather than invoking Cargo directly for a component build. The caller
depends on tool clients generated from the provider metadata during the application build.
