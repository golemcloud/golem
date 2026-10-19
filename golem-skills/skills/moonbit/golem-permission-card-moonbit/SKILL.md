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

- `@permissions.PermissionCard` — a concrete persistent card held by this agent: every grant
  names concrete owners and resources. It has no public constructor and no JSON codec. You get
  cards from the host (`@permissions.wallet()` or a derivation) and from agent or tool values that
  carry a card.
- `@permissions.PolymorphicPermissionCard` — a template persistent card. Its grants may contain
  slot variables (`?agent`, `?env`, ...), which the host resolves for each holder. It has the same
  operations and rules as `PermissionCard`; its `derive_child` returns a `PersistentCard`.
- `@permissions.ScopeCard` — an invocation-local scope card from `derive_scope`, or lent to the
  current invocation by its caller. Only scope cards attach to RPC calls, and a scope card cannot
  be revoked, installed, used as the parent of a persistent card, or passed as an agent or tool
  value.
- `@permissions.WalletCard` — one entry of `wallet()`: `Concrete(PermissionCard)`,
  `Template(PolymorphicPermissionCard)` or `Scope(ScopeCard)`. The host labels each card.
- `PatternGrant { class, owner, recipient, verb, resource_id }`, `CardView` (the four grant lists),
  `Bounds`, `Lifespan`, `Timestamp`, `WalletVersionToken`, `KernelModule` and `Holder` are plain
  data. Nothing turns them into a card; derivation inputs are requests that the host validates.
- `CardId` is opaque. It supports only `==` and hashing, so you can compare `card.id()` with
  `other.parents()`. It has no constructor, no UUID or string accessor, and no parser. Printing,
  debugging and JSON output show `CardId(<redacted>)`.

The three card types are distinct. No operation turns one into another, a schema value of a held
card decodes only into the card's own type, and user code cannot build a `WalletCard`. A card that
moves into a wire tree is no longer held, so the type that decodes the tree decides its kind, as for
a card that an agent receives. The host keeps the real kind of every card.

## Operations

| Kind | Operation |
|------|-----------|
| Borrow the card | `card.id()`, `card.parents()`, `card.expires_at()`, `card.inspect()`, `card.derive_child(bounds, lifespan?)` |
| Consume the card | `card.revoke() -> UInt`, `card.install(holder)`, `card.drop()` |
| Scope card | `scope.id()`, `scope.parents()` (root card ids), `scope.expires_at()` (earliest root expiry), `scope.inspect()`, `scope.drop()`, `scope.is_moved()` |
| Module level | `wallet() -> Array[WalletCard]`, `wallet_version()`, `derive_from_wallet(bounds, lifespan?)`, `derive_scope(parents, bounds, template_parents?, scope_parents?)`, `list_modules()`, `validate_grant(grant)`, `drop_all(wallet_cards)` |

The card rows apply to both `PermissionCard` and `PolymorphicPermissionCard`.

`derive` is a MoonBit keyword, so the card method is `derive_child`.

Persistent derivation returns `PersistentCard`, with `Concrete(PermissionCard)` and
`Template(PolymorphicPermissionCard)` cases labelled by the host. Match the case before passing
the card through a concrete or template schema. The sum supports the common card operations.

Persistent derivation accepts owner slots such as `?agent` and `?env`.
Slots in resource IDs are not supported and raise `InvalidPattern`.
Child slots name the eventual installed holder, not the deriving caller.
Every possible child instantiation must narrow the
parent resolved for the caller. The host stores the original slots; the universal envelope is
proof only. Explicit derivation inherits the parent's exact denials and adds requested denials;
wallet derivation takes complete bounds. Slot-bearing denials cannot prove universal attenuation,
so the host may conservatively refuse a request; concrete inherited denials remain.
Scope cards stay concrete: requested slots raise `InvalidPattern` with an explicit message.
For a slot-bearing tool owner, the host does not resolve aliases against caller metadata and
rejects metadata-dependent short flags. Use primary names.

A template card can be a parent: the host resolves its slot variables for this agent before it
checks the request. The host accepts a template parent of
`derive_child`, and any persistent parent of `derive_scope`, only when the card is in this agent's
wallet; a card that you derived, or received as an agent or tool value, fails there with
`NotPermitted`.

Expected failures raise `@permissions.PermissionError` (one case per WIT `permission-error` case,
with `kind()` and `message()`). `inspect`, `derive_child`, `derive_scope` and `install` on a card
that is revoked or expired raise `CardRevoked` or `CardExpired`, so `inspect` detects revocation.
Host traps stay traps: `id`, `parents` and `expires_at` trap on an unknown handle, and on a card
that is revoked or expired.

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
- `wallet()` returns new owned handles. Release the ones you do not keep with `entry.drop()`,
  `card.drop()` or `@permissions.drop_all(wallet_cards)`. Dropping releases the handle; it does not
  revoke the card.
- In a callee that holds `card:inspect`, `wallet()` also returns the scope card that the caller
  lent to the current invocation, as a `Scope` entry. Use it like any `ScopeCard`; the host
  retires it when the invocation ends.

```moonbit
let cards = @permissions.wallet()
defer @permissions.drop_all(cards)
let template_ids = []
for card in cards {
  match card {
    Concrete(_) => ()
    Template(template) => template_ids.push(template.id())
    Scope(scope) => ignore(scope.expires_at())
  }
}
```

### Lifespans

`Lifespan::iso8601("P1DT2H")` or `Lifespan::from_duration(@schema.Duration::new(nanos))`. The host
accepts only the `PnDTnHnMnS` form (days, hours, minutes, seconds) and rejects year, month and
week units with `InvalidPattern`. `from_duration` raises `InvalidPattern` for a zero or negative
duration. A child of a card that expires needs a lifespan that ends no later than the parent.

### Install targets

`install` currently accepts only `Holder::Agent(@types.AgentId)`. `Account` and `App` targets fail
with `NotPermitted`. A card whose grants do not cover the recipient fails with
`InstallRecipientMismatch`. Installing a template card installs a concrete card: the host resolves
the slot variables for the target.

## Scope cards on RPC calls

`derive_scope(parents, bounds, template_parents?, scope_parents?)` derives an invocation-local
`ScopeCard` that the host never persists. `parents` are concrete persistent cards,
`template_parents` are template cards and `scope_parents` are scope cards of the current
invocation. `scope.expires_at()` is the earliest expiry of the root cards, or `None` when no root
card expires; the scope card stops authorizing then. Attach the scope card to an awaited RPC call
to lend its authority to the callee for that call only. Attaching borrows the scope card; it is
not consumed.

A scope card is valid only during the invocation that derived or received it; after it the host
retires the handle. If you keep one, or a client whose default scope card is one, for a later invocation, the
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

`PermissionCard` and `PolymorphicPermissionCard` implement the schema traits, so they can be a
method parameter, a return value, or a field of a `#derive.golem_schema` type. A `PermissionCard`
declares `permission-card { polymorphic: false }` and a `PolymorphicPermissionCard` declares
`permission-card { polymorphic: true }`. A received card becomes the type that the parameter or
return value declares, and the host refuses an agent call input or output whose card does not match
the declared kind. Generated clients
map `polymorphic: true` to `PolymorphicPermissionCard`.

## Raw bindings

The SDK keeps its card holders in an internal package that user code cannot import, so user code
cannot build a holder or read its raw handle. The generated raw bindings under
`golemcloud/golem_sdk/interface/golem/...` remain importable, and their `@types.PermissionCard`
is a plain wrapper around a resource index. Code can put such an index into a raw wire tree and
decode it with `@schema.from_wire` or `schema_value_from_wit`. Decoding rejects an index that any
card already holds, so it cannot alias a held card, and a schema value of a held card decodes only
into that card's own type. An index that no
card holds decodes into a card. This gains no authority: the host traps on an unknown handle, and
a known one carries only authority that the agent already has. Do not use the raw bindings for
cards.
