# Scala SDK integration component

This directory owns the Scala.js component used by the Scala SDK integration tests and by the
worker-executor's real Scala tool-streaming round trip.

Build it from this directory with:

```shell
../../target/debug/golem-cli --preset release build --yes --skip-check --force-build
```

When the workspace target directory is redirected, resolve `golem-cli` through the workspace Cargo
metadata rather than assuming `../../target`.
