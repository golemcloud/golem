# golem-rust

This repository contains Rust crates that help writing [Golem](https://golem.cloud) programs.

## golem-rust

The `golem-rust` crate contains Rust wrappers for Golem's runtime API, including
the [transaction API](https://learn.golem.cloud/docs/transaction-api).

The `golem-rust` crate uses the shared `golem-rust-macro` and
`golem-tool-metadata` authoring crates from the repository's root workspace.

## Agent implementations

Traits annotated with `#[agent_definition]` must be implemented with
`#[agent_implementation]`. A plain `impl AgentTrait for Type` now fails during
`cargo check` with a missing hidden item named
`agent_implementation_annotation`, which points to the forgotten annotation.
The post-build `discover-agent-types` check remains the fallback for detecting
agent definitions that have no implementation anywhere.
