# Agent reflection acceptance matrix

This checklist tracks the contract in draft PRs #3873–#3876. A focused SDK unit test, a compiler check, and a host-backed deployment are different evidence. A combination is complete only after its applicable behavior has passed against a deployed worker on the current branch.

## Surface matrix

`F` means focused SDK tests passed, with host verification pending. `C` means type/build checks passed, with runtime verification pending. `P` means pending. `U` means intentionally unavailable by the API contract. JSON means canonical JSON accepted by reflection; native means a schema value or WIT tree. Typed means a language-level method contract.

| Level | Caller contract | Style | TypeScript | Effect | Rust | Scala | MoonBit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Generated | Complete | Typed | F | F | C | C | C |
| 2. Caller-authored | Complete | Typed | F | F | C | F | F |
| 2. Caller-authored | Binding-only | Typed | F | F | C | F | F |
| 3. Reflected | Complete deployed schema | JSON | F | F | C | F | F |
| 3. Reflected | Complete deployed schema | Native | F | F | C | F | F |
| 4. Dynamic | Binding-only | Native | F | F | C | F | F |

Every other level × contract × style combination is `U`: generated and caller-authored methods use typed language values; reflected clients use the complete schema published by the deployed type rather than a caller-authored partial contract; dynamic clients deliberately have no schema to validate canonical JSON. Native tool reflection is `U` in Effect because Effect does not expose that API.

## Behavior checklist

| Behavior | Local evidence | Host evidence still required |
| --- | --- | --- |
| Principal-scoped identity: only caller-supplied constructor fields appear in IDs; host-produced IDs round-trip through complete and reflected bindings | Shared `golem-common` regression, TypeScript and Effect focused checks | Deployment with a host-injected principal, both bindings, and repeated invocation from one durable caller in each SDK |
| Durable, ephemeral, and known phantom lifecycle | Factory and binding unit checks in TypeScript, Effect, Scala, and MoonBit; Rust SDK and test targets compile | Final ephemeral identity from invocation metadata, one-shot known phantom, durable resume, and duplicate invocation against a deployed target |
| Required, optional, defaulted, unknown, invalid, and secret config | Local negative/positive tests in TypeScript, Effect, Scala, and MoonBit; Rust config checks compile | New-worker effective config, host-provisioned secrets, missing-required rejection, and an existing durable worker retaining its persisted initial config |
| Local rejection before an RPC opens | TypeScript, Effect, Scala, and MoonBit focused checks | Deployed callers with a connection/open counter or equivalent host observation |
| Malformed declared remote outputs; awaited and pending calls | TypeScript, Effect, and Scala focused checks; Rust error surface compiles | Deployed mismatch fixtures across awaited and pending paths, including unexpected unit values |
| Stream restrictions and Effect scope/interruption cleanup | Existing SDK focused tests, including Effect interruption and scope tests | Deployed stream and interruption scenarios |
| Canonical record keys, float narrowing, U32, safe 64-bit JSON bounds, Binary, Datetime, Duration, and Quantity | SDK JSON tests in TypeScript, Scala, and MoonBit; Effect schema tests | Cross-SDK serialization and invocation round trips |

Focused tests and compiler checks do not turn an `F` or `C` cell into a host-backed pass. The available host test for RPC-supplied config targets TS/Rust workers but does not cover reflected callers. The broader documentation/site audit and publication are tracked separately.
