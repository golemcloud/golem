# Release tooling tests

This standalone Cargo workspace tests the repository's release workflows and
helper scripts. It does not build or invoke the Golem CLI, SDKs, or built-ins.
The fixtures run the workflow steps and cargo-make tasks against temporary Git
repositories and local publication stores, with external services stubbed out.

Run from the repository root:

```shell
cargo test --locked --manifest-path .github/scripts/release-tooling-tests/Cargo.toml
```

The tests require a Unix environment with Rust, cargo-make, Git, Bash, jq,
Node.js, and Python 3.
CI runs them in the **Test release tooling** step, alongside the Node.js
release-channel tests. This workspace uses the standard Rust test harness and
does not participate in the root workspace or CLI integration shards.
