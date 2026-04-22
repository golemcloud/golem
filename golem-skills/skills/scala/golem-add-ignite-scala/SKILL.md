---
name: golem-add-ignite-scala
description: "Explaining the current Apache Ignite limitation in the Scala SDK. Use when the user asks to connect to Ignite from Scala agent code or use golem:rdbms/ignite2 from the Scala SDK."
---

# Apache Ignite in the Scala SDK

The Scala SDK does not currently expose `golem:rdbms/ignite2@1.5.0` in generated Scala applications.

## Current Limitation

- `sdks/scala/wit/main.wit` imports PostgreSQL and MySQL, but not Ignite.
- `golem.host.Rdbms` currently provides Scala wrappers only for Postgres and MySQL.
- Because of that, a normal Scala Golem app cannot use Ignite today just by adding source code.

## If the User Needs Ignite Anyway

The SDK has to be extended first:

1. Add `import golem:rdbms/ignite2@1.5.0;` to `sdks/scala/wit/main.wit`.
2. Regenerate the Scala guest runtime with `sdks/scala/scripts/generate-agent-guest-wasm.sh`.
3. Add Scala.js facade types and `golem.host.Rdbms.Ignite` wrappers alongside the existing Postgres and MySQL wrappers.

Until that SDK work is done, steer Scala users toward PostgreSQL or MySQL instead of attempting to generate broken Ignite code.
