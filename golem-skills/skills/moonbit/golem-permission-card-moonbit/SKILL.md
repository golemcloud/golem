---
name: golem-permission-card-moonbit
description: "Managing permission cards from a MoonBit Golem agent with the `@permissions` module. Use when the user asks to derive, inspect, revoke, install, or pass permission cards, read the card wallet, attach a scope card to an RPC call, or delegate authority to another agent."
---

# Permission cards in MoonBit

A permission card is unforgeable delegated authority. The host owns every card; an agent holds
cards as opaque handles and asks the host to derive narrower cards, inspect them, revoke them, or
install them on another agent. The MoonBit SDK exposes this through the `@permissions` package.

Import it in the component's `moon.pkg` (the `agents` code generator already adds it):

```moonbit
import {
  "golemcloud/golem_sdk/permissions",
}
```

## Cards and plain data

- `@permissions.PermissionCard` — a persistent card held by this agent. It has no public
  constructor and no JSON codec. You get cards from the host (`@permissions.wallet()` or a
  derivation) and from agent or tool values that carry a card.
- `@permissions.ScopeCard` — an invocation-local scope card from `derive_scope`. It is a separate
  type: only scope cards attach to RPC calls, and a scope card cannot be revoked, installed, used
  as the parent of a persistent card, or passed as an agent or tool value.
- `PatternGrant { class, owner, recipient, verb, resource_id }`, `CardView` (the four grant lists),
  `Bounds`, `Lifespan`, `Timestamp`, `WalletVersionToken`, `KernelModule` and `Holder` are plain
  data. Nothing turns them into a card; derivation inputs are requests that the host validates.
- `CardId` is opaque. It supports only `==` and hashing, so you can compare `card.id()` with
  `other.parents()`. It has no constructor, no UUID or string accessor, and no parser. Printing,
  debugging and JSON output show `CardId(<redacted>)`.

## Operations

| Kind | Operation |
|------|-----------|
| Borrow the card | `card.id()`, `card.parents()`, `card.expires_at()`, `card.inspect()`, `card.derive_child(bounds, lifespan?)` |
| Consume the card | `card.revoke() -> UInt`, `card.install(holder)`, `card.drop()` |
| Scope card | `scope.id()`, `scope.parents()` (root card ids), `scope.expires_at()`, `scope.inspect()`, `scope.drop()`, `scope.is_moved()` |
| Module level | `wallet()`, `wallet_version()`, `derive_from_wallet(bounds, lifespan?)`, `derive_scope(parents, bounds, scope_parents?)`, `list_modules()`, `validate_grant(grant)`, `drop_all(cards)` |

`derive` is a MoonBit keyword, so the card method is `derive_child`.

A grant that `inspect` returns from a template card can carry slot variables such as `?agent` or
`?env` in its owner. Pass it to `derive_child`, `derive_from_wallet` or `derive_scope` unchanged:
the host resolves the slots for the calling agent. You never need to build an owner path yourself.

Expected failures raise `@permissions.PermissionError` (one case per WIT `permission-error` case,
with `kind()` and `message()`). `CardRevoked` reports a card that the host knows to be revoked; a
cached handle can still resolve after its card is revoked, so do not treat a successful call as
proof that a card is live. Host traps stay traps: `id`, `parents` and `expires_at` trap on an
unknown handle.

```moonbit
pub fn MyAgent::delegate(self : Self, target : @types.AgentId) -> Result[UInt, String] {
  ignore(self)
  let grant : @permissions.PatternGrant = {
    class: "card",
    owner: "*",
    recipient: "*",
    verb: "inspect",
    resource_id: "*",
  }
  try {
    // The four lists of `Bounds` are the complete requested lattice here.
    let card = @permissions.derive_from_wallet(
      @permissions.Bounds::new(lower_positive=[grant]),
    )
    defer card.drop()
    // For `derive_child`, the positive lists are the parent grants to retain
    // and the negative lists are the denials to add.
    let child = card.derive_child(
      @permissions.Bounds::new(lower_positive=card.inspect().lower_positive),
      lifespan=@permissions.Lifespan::iso8601("PT1H"),
    )
    child.install(Agent(target)) // moves `child`
    Ok(card.revoke()) // moves `card`; the deferred drop does nothing
  } catch {
    error => Err(error.to_string())
  }
}
```

### Moving and releasing cards

- `revoke` and `install` mark the card as moved **before** they call the host, because the host
  drops the handle even when the call fails. `card.is_moved()` reports this.
- Passing a card as an agent or tool argument or return value also moves it. When the value fails
  to encode, the host never takes the card and you keep it.
- Using a moved card aborts the invocation.
- `wallet()` returns new owned handles. Release the ones you do not keep with `card.drop()` or
  `@permissions.drop_all(cards)`. Dropping releases the handle; it does not revoke the card.
- In a callee, `wallet()` also returns the scope card that the caller lent to the current
  invocation, typed as a `PermissionCard`. The host rejects `revoke`, `install` and
  `derive_child` on it, and a rejected `revoke` or `install` still consumes the handle. Sending it
  as an agent or tool value fails, and the host retires it when the invocation ends.

### Lifespans

`Lifespan::iso8601("P1DT2H")` or `Lifespan::from_duration(@schema.Duration::new(nanos))`. The host
accepts only the `PnDTnHnMnS` form (days, hours, minutes, seconds) and rejects year, month and
week units with `InvalidPattern`. `from_duration` raises `InvalidPattern` for a zero or negative
duration. A child of a card that expires needs a lifespan that ends no later than the parent.

### Install targets

`install` currently accepts only `Holder::Agent(@types.AgentId)`. `Account` and `App` targets fail
with `NotPermitted`. A card whose grants do not cover the recipient fails with
`InstallRecipientMismatch`.

## Scope cards on RPC calls

`derive_scope(parents, bounds, scope_parents?)` derives an invocation-local `ScopeCard` that the
host never persists. `parents` are persistent cards and `scope_parents` are scope cards of the
current invocation. Attach the scope card to an awaited RPC call to lend its authority to the
callee for that call only. Attaching borrows the scope card; it is not consumed.

A scope card is valid only during the invocation that derived it; after it the host retires the
handle. If you keep one, or a client whose default scope card is one, for a later invocation, the
host denies awaited calls that attach it and discards their whole input, argument cards included.
Typed clients (`@rpc` and generated) then abort with `RPC denied`; reflection clients raise
`Remote(Denied)`. `derive_scope` with a retired scope card as a parent fails with `NotPermitted`.
Dropping a retired scope card is safe.

```moonbit
let scope = @permissions.derive_scope([parent], @permissions.Bounds::new(lower_positive=[grant]))
let client = WorkerAgentClient::get("worker-1")
defer client.drop()
// Per call:
let result = client.process(input, scope_card=scope)
// Or as the client default for every awaited call that passes no `scope_card`:
client.set_default_scope_card(Some(scope))
let other = client.process(input)
client.set_default_scope_card(None)
```

Only awaited calls accept a scope card, and only a `ScopeCard`: a `PermissionCard` does not
type-check as a scope card. A dropped scope card aborts the call before the input is encoded. `trigger_*`, `schedule_*` and `schedule_cancelable_*`
methods never attach one. The same `scope_card?` option and `set_default_scope_card` exist on
`@rpc.AgentClient` and on the reflection clients (`ReflectedAgentClient`, `DynamicAgentClient`,
`FullAgentClient`, `MethodOnlyAgentClient`). Tool clients do not take scope cards.

## Cards in agent and tool signatures

`PermissionCard` implements the schema traits, so it can be a method parameter, a return value, or
a field of a `#derive.golem_schema` type. Its schema is a concrete (non-polymorphic)
`permission-card` node.

## Raw bindings

The SDK keeps its card holders in an internal package that user code cannot import, so user code
cannot build a holder or read its raw handle. The generated raw bindings under
`golemcloud/golem_sdk/interface/golem/...` remain importable, and their `@types.PermissionCard`
is a plain wrapper around a resource index. Code can put such an index into a raw wire tree and
decode it with `@schema.from_wire` or `schema_value_from_wit`. Decoding rejects an index that a
`PermissionCard` or `ScopeCard` already holds, so it cannot alias a held card. An index that no
card holds decodes into a card. This gains no authority: the host traps on an unknown handle, and
a known one carries only authority that the agent already has. Do not use the raw bindings for
cards.
