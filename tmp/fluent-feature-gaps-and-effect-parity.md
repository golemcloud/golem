# Fluent SDK feature gaps vs base — + effect-golem parity audit

Surfaced while porting the TS test-components to fluent (Track C Stage 5): 4 capabilities the **base/decorator**
SDK had that the **fluent** SDK lacked. Decision (user): add all 4 to fluent. This doc is the parity audit —
whether **effect-golem** (per line) already has each, vs old base — so effect can reach parity too.

**Key finding (CORRECTED):** the wire *data* (`SchemaValueTree` / `AgentType` metadata) needs no wasm change — BUT
all 4 features add/modify SDK **runtime code** (`s.principal`, `clientFor.abortable`, config encoding, `resolveReadOnly`),
and the ts component template **externalizes `@golemcloud/golem-ts-sdk`** (rollup `external`), resolving it to the SDK
runtime **embedded in `agent_guest.wasm`**. So the `agent_guest.wasm` MUST be rebuilt (`pnpm run build-agent-template`)
for any of the 4 to exist at component runtime. Symptom when stale: `s.principal()` (or `.abortable`, etc.) →
quickjs "not a function" trap during agent-type extraction. (The initial "no rebuild" assumption was wrong.)

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

## Fluent implementation status — DONE (commit `5826159bd`; 283 tests, clean build, 0 lint errors)
- [x] F4 readOnly cache policies — `method.ts` + `runtime.ts`; fixed the `no-cache`→`until-write` default regression
- [x] F2 abortable/cancelable RPC — `client.ts` (`.abortable`/`.scheduleCancelable`)
- [x] F3 config-on-RPC — `client.ts` (`clientFor(def)(id, phantomId?, config?)`, secret overrides rejected)
- [x] F1 `s.principal()` marker — `schema/markers.ts` + `httpTypes.ts` (+ 5 round-trip tests)

## ⚠️ 5th gap discovered (during the Stage 5 re-port) — NEEDS A DECISION
**Principal as an auto-injected method *input* parameter.** F1 added `s.principal()` as a *data value*
(return/nested), which works. But the base SDK also let a method declare a `Principal` *input parameter* that
consumes NO wire field and is auto-injected from the caller (per-call principal). Fluent doesn't wire this:
- `runtime.ts` `encodeInput` hardcodes every input field `source: { tag: 'user-supplied' }` — never emits
  `auto-injected(principal)` (which the host keys off: `golem-worker-executor/.../invocation.rs` injects the
  principal into the `AutoInjected(Principal)` field).
- `runtime.ts` `invoke(...)` ignores its per-call `_principal` arg (doesn't splice it into handler args).
- `httpTypes.ts` bodyless-unbound check doesn't exclude a `principal` input, so an unbound `s.principal()` input
  on a GET is a compile error.
`this.getPrincipal()` returns the INIT-time principal (fine for durable agents constructed per-caller, wrong for
a shared durable agent). **Re-port workaround:** `PrincipalAgent` uses `phantomAgent: true` (fresh instance per
HTTP request → `this.getPrincipal()` = that request's principal). This passes `agent_http_principal_ts.rs` (asserts
only HTTP responses) but is a durability-semantics deviation. **Decision: wire the 3 gaps above (faithful), or keep
the phantomAgent workaround?** effect-golem HAS auto-inject (via the `Principal` `Context.Service`).

## Remaining follow-ups
- [x] **Effect-parity backport DONE** (local commits, unpushed):
  - `golem-1.6`: F4 fully wired `0d36725`, F1 `d24b361` — 670 tests green.
  - `main-naming-backport`: F1 `d2752b8` (adapted to the witTree wire model), F4 SDK-surface-only `6063227`
    — **F4 wire emission impossible on 1.5.0** (`golem:agent/common@1.5.0` `agent-method` has no `read-only`
    field / no `ReadOnlyConfig` type), so `readOnly` is accepted but not emitted there. 670 tests green.
  - F2+F3 already existed in effect (both lines) — no action.
- [x] **Track C Stage 5 re-port DONE** (`83df7db10`): 4 agents restored with the new features. wasm rebuild +
  integration-test run pending (needs `target/debug/golem-cli`; source API-verified + SDK features tested).
- [ ] **5th gap decision** (Principal input auto-injection): wire it, or keep the `phantomAgent` workaround.
