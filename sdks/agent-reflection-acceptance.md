# Agent reflection acceptance matrix

This checklist tracks the contract in draft PRs #3873–#3876. A focused SDK unit test, a compiler check, and a host-backed deployment are different evidence. A combination is complete only after its applicable behavior has passed against a deployed worker on the current branch.

## Surface matrix

`F` means focused SDK tests passed, with host verification pending. `C` means type/build checks passed, with runtime verification pending. `P` means pending. `U` means intentionally unavailable by the API contract. `H+` means a positive deployed call passed, with negative host cases pending. JSON means canonical JSON accepted by reflection; native means a schema value or WIT tree. Typed means a language-level method contract.

| Level | Caller contract | Style | TypeScript | Effect | Rust | Scala | MoonBit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Generated | Fully defined client | Typed | F | F | C | C | C |
| 2. Caller-authored | Fully defined client | Typed | F | F | C | F | F |
| 2. Caller-authored | Method-only client | Typed | F | F | C | F | F |
| 3. Reflected | Fully defined deployed schema | JSON | H+ | H+ | C | F | F |
| 3. Reflected | Fully defined deployed schema | Native | H+ | H+ | C | F | F |
| 4. Dynamic | Method-only client | Native | H+ | H+ | C | F | F |

Every other agent level × contract × style combination is `U`: generated and caller-authored methods use typed language values; reflected clients use the fully defined schema published by the deployed type rather than a caller-authored method-only contract; dynamic clients deliberately have no schema to validate canonical JSON.

## Native tool reflection matrix

The tool matrix covers all five SDKs, including Effect. A tool has a command path and stream attachments rather than an agent identity or lifecycle. `P` remains pending until focused tests and a deployed-tool proof pass for that SDK and row.

| Level | Caller contract | Style | TypeScript | Effect | Rust | Scala | MoonBit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Exact definition or generated | Fully defined client | Typed | H+ | H+ | P | P | P |
| 2. Caller-authored | Fully defined or command subset | Typed | P | H+ | P | P | P |
| 3. Reflected | Fully defined deployed tool schema | JSON | H+ | H+ | P | P | P |
| 3. Reflected | Fully defined deployed tool schema | Native | H+ | H+ | P | P | P |
| 4. Dynamic | Name and command path only | Native | H+ | H+ | P | P | P |

Other tool level × contract × style combinations are `U`: typed clients require caller-owned codecs, reflected JSON requires discovered schemas, and dynamic calls deliberately have no schema for JSON packing. No SDK is intentionally unavailable for native tool reflection.

### Tool behavior evidence

| Behavior | Focused SDK evidence required | Host-backed evidence required |
| --- | --- | --- |
| Accessible discovery, aliases, canonical paths, namespace-only nodes, stale snapshots | Metadata and missing/inaccessible lookup tests in all five SDKs | Deploy and revise a bound tool; prove discovery is caller-filtered and invocation does not silently refresh or retry |
| Inherited globals, option carriers, repeatable values, defaults, refinements, and constraints | Shared descriptor fixtures and local rejection before RPC in all five SDKs | Invoke a deployed tool from each SDK with positive and negative canonical inputs |
| Unit/value results, malformed outputs, declared custom and structural remote errors | Awaited and pending tests in all five SDKs | Deployed mismatch/error fixtures with original nested errors preserved |
| Required/optional stdin and stdout, concurrent collection, trigger restrictions | Scoped stream, cancellation, early-close, and cleanup tests in all five SDKs | Deployed streaming invocation and cancellation, including Effect interruption |

No tool matrix cell is complete solely from a mock-host test.

Union JSON Schema exports combine each discriminator condition with its branch schema. Regex
discriminators are emitted unchanged; external validators may use a different regex engine from
the SDK runtime, so equivalence across engines is not guaranteed.

The TS and Effect `H+` cells were exercised by the deployed cross-SDK fixture's
`RUN_TOOL_REFLECTION_ONLY=1` path. Each caller discovered the other SDK's tool and invoked its
required-stdin/required-stdout command with canonical JSON, a reflected native schema value,
and a caller-owned typed value through a dynamic client. Generated typed clients in both SDKs
and Effect's caller-authored typed client also passed. Structured results and byte streams
matched in every case. The full cross-SDK harness was
also attempted, but its earlier `callEffectStream` case stopped progressing before it reached
the tool checks. The targeted path ran against a fresh local deployment and passed. Its local
build required a current checkout-built `golem` CLI and a temporary Node preload to unref
Rollup's lingering file watchers; neither changes the SDK contract.

The TS and Effect reflected callers also attempted an invalid declared string argument in the
deployed fixture and reported local input rejection. Focused mock-host tests assert that these
invalid calls do not open an RPC.

The same deployed `RUN_TOOL_REFLECTION_ONLY=1` path invoked optional, no-default tool options
in both directions: a TS reflected caller used the Effect provider, and an Effect reflected
caller used the TS provider. Each invoked omitted (`null`) and supplied (`"supplied"`) values
through canonical JSON and schema-native inputs. All eight calls returned the expected
`omitted` or `supplied` result. Focused model tests also cover both carrier values and the
host's conversion of optional options and positionals into command arguments.

The deployed cross-SDK fixture's `RUN_AGENT_REFLECTION_ONLY=1` path invoked `TsPeer` twice from
the same durable Effect caller, parsed the host-produced remote ID, resolved its deployed type,
and invoked it again through a schema-free dynamic client bound to that ID.
It also invoked a reflected ephemeral phantom and checked its final ID. Reflected JSON input and
native nonfinite output passed for TS and Rust peers, while the declared JSON nonfinite output was
rejected. This path isolates agent reflection from the earlier large-stream case that stalls the
full cross-SDK harness.

The fixture's `RUN_TS_AGENT_REFLECTION_ONLY=1` path invoked `TsPeer.reflectedEffectAgent`
twice from one durable caller against a fresh deployment. It discovered and bound
`EffectSnapshotFixture`, called its declared methods through canonical JSON and native schema
values, parsed the host-produced remote ID, rebound the discovered type, and read the same
persisted state through a schema-free dynamic client. The count advanced from `0` to `1` and
then from `1` to `2`. The same deployed caller also invoked `TsPrincipalPeer`, whose constructor
declares a host-injected principal. The host-produced ID parsed into exactly one caller-supplied
tenant field. Fully defined and reflected clients, including bindings through that ID, advanced the
same worker's counter through `1, 2, 3, 4`, then `5, 6, 7, 8` on a second caller invocation.
Principal identity checks in the other SDKs and negative
host cases remain pending.

That deployment also created an `EffectFixture` worker from the reflected TypeScript factory with
a local `prefix` override. A second factory call passed a different override for the same worker;
both method results retained the initial prefix. Unknown config paths and a number where the
declared config requires a string were rejected locally. Required config without a default,
host-provisioned secrets, and the other SDK callers remain pending.

## Behavior checklist

| Behavior | Local evidence | Host evidence still required |
| --- | --- | --- |
| Principal-scoped identity: only caller-supplied constructor fields appear in IDs; host-produced IDs round-trip through fully defined and reflected bindings | Shared `golem-common` regression, TypeScript and Effect focused checks | TypeScript deployment passed; other SDK callers remain pending |
| Durable, ephemeral, and known phantom lifecycle | Factory and binding unit checks in TypeScript, Effect, Scala, and MoonBit; Rust SDK and test targets compile | Final ephemeral identity from invocation metadata, one-shot known phantom, durable resume, and duplicate invocation against a deployed target |
| Required, optional, defaulted, unknown, invalid, and secret config | Local negative/positive tests in TypeScript, Effect, Scala, and MoonBit; Rust config checks compile | Reflected TS new-worker override, existing-worker persistence, unknown and invalid local rejection passed; host-provisioned secrets, missing-required rejection, and other SDK callers remain pending; direct TS/Rust RPC new-worker overrides and persisted existing-worker config passed |
| Local rejection before an RPC opens | TypeScript, Effect, Scala, and MoonBit focused checks | Deployed callers with a connection/open counter or equivalent host observation |
| Malformed declared remote outputs; awaited and pending calls | TypeScript, Effect, and Scala focused checks; Rust error surface compiles | Deployed mismatch fixtures across awaited and pending paths, including unexpected unit values |
| Stream restrictions and Effect scope/interruption cleanup | Existing SDK focused tests, including Effect interruption and scope tests | Deployed stream and interruption scenarios |
| Canonical record keys, float narrowing, U32, safe 64-bit JSON bounds, Binary, Datetime, Duration, and Quantity | SDK JSON tests in TypeScript, Scala, and MoonBit; Effect schema tests | Cross-SDK serialization and invocation round trips |

The host-backed `ts_reflection_discovers_binds_and_invokes_durable_agent` test discovers a Rust Counter from a TypeScript caller, invokes it twice through reflected native-schema clients, and checks the same persisted value after rebinding by agent ID. `ts_reflected_ephemeral_invocation_returns_final_metadata` verifies a known ephemeral phantom and its final ID and idempotency key. `ts_ephemeral_final_identity_cannot_be_reused` checks recoverable rejection through a fully defined caller-authored client; `ephemeral_rpc_invocations_get_distinct_final_identities` checks distinct final IDs through repeated invocation from one durable caller. These tests passed with freshly rebuilt `agent-rpc` and `agent-counters` components. They do not cover host-injected principal fields or the other SDKs' reflected lifecycle calls.

The host-backed `agent_config::rpc` cases `rpc_provided_config_overrides_defaults`, `rpc_can_start_agent_by_providing_config_missing_in_defaults`, and `rpc_does_not_override_values_of_existing_agent` passed for TypeScript and Rust workers on both SQLite and Postgres (12 cases total). They used freshly rebuilt `agent-sdk-ts` and `agent-sdk-rust` components and serialized test execution because the local service harness shares ports. These direct RPC tests establish effective creation config and persisted initial config, but do not cover reflected callers, secret provisioning, or negative validation.

Focused tests and compiler checks do not turn an `F` or `C` cell into a host-backed pass. The broader documentation/site audit and publication are tracked separately.
