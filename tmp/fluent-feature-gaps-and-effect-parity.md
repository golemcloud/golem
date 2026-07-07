# Fluent SDK feature gaps vs base — + effect-golem parity audit

Surfaced while porting the TS test-components to fluent (Track C Stage 5): 4 capabilities the **base/decorator**
SDK had that the **fluent** SDK lacked. Decision (user): add all 4 to fluent. This doc is the parity audit —
whether **effect-golem** (per line) already has each, vs old base — so effect can reach parity too.

**Key finding:** none of the 4 needs the embedded `agent_guest.wasm` rebuilt — all are pure SDK-side (TypeScript)
changes flowing through the existing `SchemaValueTree` / `AgentType`-metadata channels the guest forwards verbatim.

## Parity table
| Feature | Base | Fluent (before) | effect `golem-1.6` | effect `main-naming-backport` (1.5) | Reference for impl |
|---|---|---|---|---|---|
| 1. `Principal` as a data schema (`s.principal()`) | ✅ variant via `mapPrincipal` | ❌ only `this.getPrincipal()` | ❌ `Context.Service` only | ❌ same | base (`mapPrincipal`) — **effect also needs this** |
| 2. Abortable / cancelable RPC | ✅ `.abortable`/`.scheduleCancelable` | ❌ only `()/.trigger/.schedule` | ✅ fiber-interrupt + `ScheduledInvocation.cancel` | ✅ same | effect `Client.ts`/`RpcClient.ts` + base `clientGeneration.ts` |
| 3. Config-on-RPC (`getWithConfig`) | ✅ `serializeRpcConfigObject` | ❌ `new WasmRpc(…, [])` | ✅ `GetOptions.overrides`/`buildAgentConfig` | ✅ same | effect `Client.ts` `buildAgentConfig` + base |
| 4. `readOnly` cache policies | ✅ `no-cache`/`until-write`/`ttl` + `usesPrincipal` | ❌ bool→forced `no-cache` (wrong default) | ❌ no readOnly/cache field | ❌ same | base `decorators/readOnly.ts` — **effect also needs this** |

## Effect-parity TODO (backport to effect-golem after fluent, both lines unless noted)
- **Feature 1 (Principal schema):** effect has NO value schema for Principal (only the `Principal` `Context.Service`).
  → add a Principal `Schema`/`WitCodec` (variant oidc/agent/golem-user/anonymous) to `effect-golem/src` on **both** lines.
- **Feature 4 (readOnly cache):** effect `MethodSpec` has no `readOnly`/cache field at all.
  → add a `readOnly` option + `ReadOnlyConfig` emission to effect `method`/`agent` on **both** lines. (fluent already
  leads effect even on the boolean.)
- **Feature 2 (abortable RPC):** effect ALREADY has it (fiber-interrupt `asyncInvoke` + `ScheduledInvocation.cancel`) — no action.
- **Feature 3 (config-on-RPC):** effect ALREADY has it (`GetOptions.overrides` + `buildAgentConfig`) — no action.

## Fluent implementation status (this pass)
- [ ] F4 readOnly cache policies — `src/fluent/method.ts` + `runtime.ts` (also fixes the `no-cache` default regression → `until-write`)
- [ ] F2 abortable/cancelable RPC — `src/fluent/client.ts`
- [ ] F3 config-on-RPC — `src/fluent/client.ts` (+ config encoding)
- [ ] F1 `s.principal()` marker — `src/fluent/schema/markers.ts` (+ variant codec)
- [ ] rebuild not required; then restore the un-ported test-component agents + finish Stage 5.
