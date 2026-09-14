# TypeScript tool streaming integration components

This application owns the TypeScript provider/caller pair used by the tool streaming integration
suite.

Build and copy both artifacts from this directory with:

```shell
GOLEM_TS_PACKAGES_PATH="$(cd ../../sdks/ts/packages && pwd)" ../../target/debug/golem-cli --preset release build --yes --skip-check --force-build
GOLEM_TS_PACKAGES_PATH="$(cd ../../sdks/ts/packages && pwd)" ../../target/debug/golem-cli --preset release exec copy
```

When the workspace target directory is redirected, resolve `golem-cli` through the workspace Cargo
metadata instead of assuming `../../target`.
