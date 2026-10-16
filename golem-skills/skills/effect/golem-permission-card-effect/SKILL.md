---
name: golem-permission-card-effect
description: Transfers, inspects, derives, scopes, revokes, and installs permission cards in Effect Golem agents. Use when delegated authority is part of a schema or an agent manages its cards.
---

# Permission cards in Effect

A permission card is an opaque, affine capability. The SDK has three card types in `Permissions`
from `@golemcloud/effect-golem`:

- `PermissionCard`: a concrete card. Every grant names concrete owners and resources.
- `PolymorphicPermissionCard`: a template card. Its grants may contain slot variables (`?agent`,
  `?env`, ...), which resolve for each holder.
- `ScopeCard`: an invocation-local card from `deriveScope` (see below).

The SDK has no conversion API between these types. User code cannot construct a card, parse one
from data, or serialize one. Cards come from the host (`Permissions.wallet`, `deriveFromWallet`, `derive`,
`deriveScope`) or from another call.

## Transfer a card through a schema

Declare a concrete card with `Schema.PermissionCard` (`permission-card { polymorphic: false }`) and
a template card with `Schema.PolymorphicPermissionCard` (`polymorphic: true`), and place it
directly in the input or output schema. Each schema encodes only cards of its kind, and a decoded
card takes the type that the schema declares. The host keeps the real kind of the card. Successful encoding transfers ownership: do not
use the card again after the call. If encoding a composite value fails, the SDK restores the card
so that a corrected retry can use it.

## Manage cards

Every `Permissions` function returns an Effect that requires `PermissionsClient`; the agent runtime
provides it. Expected host failures are `PermissionError` values with a `kind` such as
`"not-permitted"`, `"card-revoked"`, or `"install-recipient-mismatch"`. Moved cards and unexpected
host failures are `PermissionsHostError` values.

```ts
import { Effect, Equal } from "effect"
import { Permissions } from "@golemcloud/effect-golem"

const kvRead: Permissions.PatternGrant = {
  class: "kv",
  owner: "?env",
  recipient: "*",
  verb: "read",
  resourceId: "store.key",
}

const delegate = Effect.gen(function* () {
  const card = yield* Permissions.deriveFromWallet({
    lowerPositive: [kvRead],
    upperPositive: [kvRead],
  })
  const view = yield* Permissions.inspect(card)
  const child = yield* Permissions.derive(
    card,
    { upperPositive: view.upperPositive },
    Permissions.Lifespan.fromIso8601("P1D"),
  )
  const parentId = yield* Permissions.id(card)
  const isChild = (yield* Permissions.parents(child)).some((id) => Equal.equals(id, parentId))
  return { card, child, isChild }
})
```

- `derive(card, bounds, lifespan?)`: the positive bounds are the parent grants to retain, and the
  negative bounds are the denials to add. `derive` accepts a `PermissionCard` or a
  `PolymorphicPermissionCard` and always returns a `PermissionCard`: a template parent is resolved
  against the caller.
- `deriveFromWallet(bounds, lifespan?)` and `deriveScope(parents, bounds)` take the complete
  requested lattice. Omitted bound lists are empty.
- `Lifespan.fromIso8601("P30D")` or `Lifespan.fromDuration("1 hour")` builds a lifespan. Use days
  and time units only (`PnDTnHnMnS`); the host rejects years, months, and weeks. `fromDuration`
  rejects zero and negative durations.
- `id`, `parents`, `expiresAt`, `inspect`, `derive`, and `deriveScope` borrow the card. The `id`,
  `parents`, and `expiresAt` accessors trap in the host if the handle is unknown. A handle the
  agent already holds may keep reporting its cached view after its card is revoked, so do not use
  these accessors to detect revocation.
- `revoke(card)` returns the number of revoked cards in the subtree. `install(card, holder)` puts
  the card in another agent's wallet. Both move the card before the host call, even when the call
  fails. `release(card)` drops a card that you do not keep.
- `CardId` is opaque. Compare IDs with `Equal.equals`. Printing and logging show
  `CardId(<redacted>)`.
- `wallet` returns owned `WalletCard` entries, labelled with their kind:
  `{ _tag: "Concrete", card: PermissionCard }`,
  `{ _tag: "Template", card: PolymorphicPermissionCard }`, or `{ _tag: "Scope", card: ScopeCard }`.
  Match on `_tag` to get the typed card. Use `walletScoped` to release the cards that are still
  held when the scope closes. During an invocation that received a scope card, the list also
  contains that scope card as a `Scope` entry if the agent holds `card:inspect`.
- Build an install target with `Permissions.agentHolder(identity)` (for example from
  `defineAgentClient(...).agentId(...)`) or `Permissions.Holder.agent(agentId)`. The host rejects
  account and app holders with `not-permitted`.

## Scope cards on agent calls

`deriveScope` returns a `Permissions.ScopeCard`, a separate type from `PermissionCard`. A scope card
gives the callee extra authority for one awaited invocation. Attaching a scope card borrows it, so
the caller can use it again. A scope card supports `id`, `parents`, `expiresAt`, `inspect`,
`release`, and use as a `deriveScope` parent. It cannot be revoked, installed, derived from with
`derive`, or put in an agent-method schema. `expiresAt` of a scope card is the earliest expiry of
the root cards of this scope card, or none when no root card expires. The scope card stops
authorizing then.

```ts
const wallet = yield* Permissions.walletScoped
const scope = yield* Permissions.deriveScope(wallet.map((entry) => entry.card), {
  lowerPositive: [kvRead],
  upperPositive: [kvRead],
})
const reader = yield* Reader.client.get({ name: "reader" })
yield* reader.read.withScopeCard(scope)({ key: "key" })

const scopedReader = yield* Reader.client.get({ name: "reader" }, { scopeCard: scope })
yield* scopedReader.read({ key: "key" })
```

Only awaited calls carry a scope card. `trigger`, `schedule`, and tool calls never carry one.
`DynamicMethod.invoke` and reflected `invoke` take `{ scopeCard }` as a second argument. Generated
Effect bridges have `client.withScopeCard(scope)` and `client.method.withScopeCard(scope)(...args)`.

A scope card is valid only during the invocation that derived it. Do not keep it, or a client whose
default is it, in agent state: a later invocation's awaited calls fail with an `RpcCallError` whose
cause is `denied`. A released scope card fails the call with `ScopeCardUnavailableError` before
the input is encoded, so no argument card is moved.
