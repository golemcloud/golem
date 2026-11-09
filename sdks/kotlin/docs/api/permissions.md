# Permission cards

> Golem's capability-based authorisation: **cards** are unforgeable grants held by the runtime, and an
> agent can inspect, derive, install, revoke and lend them (`golem:permissions@0.1.0`). **Status:** ✅ Complete
> for the 1.6.0 API. Verified on a real 1.6.0 executor by running the shared Scala permission-card scenario unchanged
> (`golem-worker-executor/tests/permission_cards_scala.rs`) against a Kotlin port of its fixture.

## Overview

A card is a handle to a record held by the runtime, never the grants themselves. All operations go through
`cloud.golem.permissions`:

- `Permissions` — the agent's **wallet** (`wallet()`), `deriveFromWallet`, `deriveScope`, `selfVersion`,
  `kernelModules`, `validateGrant`.
- Card classes — `PermissionCard` (concrete), `PolymorphicPermissionCard` (template), `ScopeCard`, plus the wrappers
  `PersistentCard` (`Concrete` | `Template`, what `derive` returns) and `WalletCard` (`Concrete` | `Template` | `Scope`).

Which cards an agent starts with is **deployment configuration** (the agent type's initial permissions), not something the
SDK declares.

## Handles move

A card owns a runtime handle. The handle **moves** when the card is revoked, installed, or sent to another agent as a method
argument or result; afterwards the card is absent (`isPresent == false`) and every operation throws `IllegalStateException`.
Call `release()` on cards you keep but no longer need (handles are not reclaimed by Kotlin/Wasm GC); releasing an absent card
is a no-op.

## API

```kotlin
object Permissions {
    fun wallet(): List<WalletCard>                       // fresh handles; release the ones you do not keep
    fun selfVersion(): WalletVersionToken
    fun deriveFromWallet(bounds: Bounds, lifespan: Lifespan? = null): Either<PermissionError, PersistentCard>
    fun deriveScope(parents: List<Card>, bounds: Bounds = Bounds.empty): Either<PermissionError, ScopeCard>  // parents are borrowed
    fun kernelModules(): List<KernelModule>
    fun validateGrant(grant: PatternGrant): Either<PermissionError, Unit>
}

sealed class Card {
    val isPresent: Boolean
    val id: CardId
    val parents: List<CardId>
    val expiresAt: Timestamp?
    fun inspect(): Either<PermissionError, CardView>
    fun release()
}
// PermissionCard / PolymorphicPermissionCard add:
//   fun derive(bounds: Bounds, lifespan: Lifespan? = null): Either<PermissionError, PersistentCard>
//   fun revoke(): Either<PermissionError, Long>      // cards revoked, including descendants; moves the handle
//   fun install(target: Holder): Either<PermissionError, Unit>   // moves the handle
```

`PersistentCard` exposes the same `inspect` / `derive` / `revoke` / `install` / `release` and a `card` property; `ScopeCard` cannot be derived from.

Supporting types: `Bounds` (four `List<PatternGrant>`), `PatternGrant(className, owner, recipient, verb, resourceId)`,
`CardView`, `Holder` (`Account` | `App` | `Agent`), `Lifespan` (`Lifespan.of(2.hours)` or `Lifespan.iso8601("PT2H")`; the text is `PnDTnHnMnS` only, so `P1DT1H1M1S` and `PT2.5S` are valid and `P1Y` is not; a zero or infinite duration is rejected),
`Timestamp`, `CardId` (opaque, compares by value), `WalletVersionToken`, `KernelModule`, and `PermissionError` with its 12
cases (`NotPermitted`, `LowerBoundTooBroad`, `UpperBoundTooBroad`, `WalletMultiSourceRequired`, `CardRevoked`,
`CardExpired`, `InstallRecipientMismatch`, `UnknownResourceClass`, `InvalidPattern`, `InvalidVerb`, `InvalidOwner`,
`InvalidRecipient`). Each `PermissionError` has a `kind` and prints as `kind: message`. Using a handle that `revoke` or `install` already moved reports "already been moved", and the card prints as `(<moved>)` instead of `(<present>)`. Fallible calls return `Either` (`Left` = error), like the rest of the SDK.

## Lending a card for one call (scope cards)

A scope card gives a callee authority only while an RPC call runs, and expires with its earliest parent. The card is
**borrowed** for the call (its handle stays with you). On a typed client:

```kotlin
val scope = (Permissions.deriveScope(listOf(root), Bounds(lowerPositive = listOf(grant))) as Either.Right).value
val client = TargetRpc(SchemaValue.Record(listOf(SchemaValue.Str("target"))))

client.withScopeCard(scope) { inspectAllowed() }          // lent for the calls made inside the block
client.withDefaultScopeCard(scope)                         // lent for every later call (null stops lending)
```

On the low-level `WasmRpc`, every call (`invokeAndAwait`, `invoke`, `asyncInvokeAndAwait`, `scheduleInvocation`,
`scheduleCancelableInvocation`) takes an optional `scopeCard`, and `WasmRpc.defaultScopeCard` sets the client default.
A retired scope card carries no authority: lending it makes the call fail with an `RpcException` ("rpc call denied: …").

## Cards as agent arguments and results

A method parameter or result of type `PermissionCard` or `PolymorphicPermissionCard` becomes a `permission-card` in the
agent type's schema (`polymorphic = false` / `true`) and travels as an owned handle: sending a card moves it, and the
receiver gets a card it owns. An argument carries the card but does **not** install it in the receiver's wallet.

```kotlin
@Agent(mount = "/inbox/{name}")
class Inbox(val name: String) : BaseAgent() {
    @Endpoint(post = "/receive")
    fun receive(card: PolymorphicPermissionCard): Boolean {
        val ok = card.inspect() is Either.Right
        card.release()
        return ok
    }
}
```

## Example: derive, install, revoke

```kotlin
val root = (Permissions.wallet().first { it is WalletCard.Template }).card as PolymorphicPermissionCard
val grant = (root.inspect() as Either.Right).value.lowerPositive.first { it.className == "card" && it.verb == "inspect" }

val child = (root.derive(Bounds(lowerPositive = listOf(grant)), Lifespan.of(1.hours)) as Either.Right).value
child.install(Holder.Agent(otherAgentId))            // moves child's handle
```

## Notes

- `Card.intoSchemaValue()` is the move-into-a-schema-value primitive used by generated code; user code sends a card by
  passing it as an argument or returning it.
- `RpcException` messages are now readable (`rpc call denied: …`, `rpc target not found: …`, …) instead of the data-class text.
- The acceptance fixture is `scripts/permission-cards/PermissionCardAgents.kt` (same agent types and methods as the Scala
  fixture). To run it: build it as a component, place the wasm where the harness expects the `tool_streaming_scala` component,
  and run `cargo +1.96.0 test -p golem-worker-executor --test integration -- permission_cards_scala`.
- Not covered: cards in durable streams / HTTP routers, `schema-value-stream`.

See also: [RPC](rpc.md) · [Host API](host-api.md) · [Tools](tools.md) · [SDK README](../../README.md).
