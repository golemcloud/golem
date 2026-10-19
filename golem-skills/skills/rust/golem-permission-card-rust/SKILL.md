---
name: golem-permission-card-rust
description: "Managing permission cards (delegated authority) from Rust agents with golem_rust::permissions: deriving, inspecting, installing, and revoking cards, lending scope cards to agent calls, and passing cards through agent and tool calls. Use when the user asks about permission cards, delegated authority, card derivation, wallets, scope cards, or revoking granted access."
---

# Permission cards in Rust

A permission card is an unforgeable capability that grants authority, for example to read a
secret, call a tool, or manage other cards. The host keeps the card; the agent holds a handle to
it. `golem_rust::permissions` is the typed API over the handle.

```rust
use golem_rust::permissions::{
    self, Bounds, Holder, Lifespan, PatternGrant, PermissionCard, PermissionError,
    PolymorphicPermissionCard, ScopeCard, WalletCard,
};
```

Every operation is checked by the host against the agent's own cards: deriving needs
`card:derive`, inspecting needs `card:inspect`, revoking needs `card:revoke`, and installing needs
`card:install`. Grant these to the agent type in the manifest. A refusal is returned as
`Err(PermissionError)`; it is not a panic.

## The wallet

The wallet holds the cards installed for the agent, including its initial card. `wallet()` returns
each card as a `WalletCard`, labelled with its kind:

| Case | Card type | Card |
|------|-----------|------|
| `WalletCard::Concrete` | `PermissionCard` | every grant names concrete owners and resources |
| `WalletCard::Template` | `PolymorphicPermissionCard` | the grants may contain slot variables (`?agent`, `?env`, ...), which resolve for each holder; the agent's initial card is one |
| `WalletCard::Scope` | `ScopeCard` | the scope card lent to the current invocation, listed only if the agent holds `card:inspect` |

```rust
for card in permissions::wallet() {
    match card {
        WalletCard::Concrete(card) => { /* PermissionCard */ }
        WalletCard::Template(card) => { /* PolymorphicPermissionCard */ }
        WalletCard::Scope(card) => { /* ScopeCard */ }
    }
}
let version = permissions::wallet_version(); // changes when the wallet changes
```

The SDK offers no conversion between the three card types. `WalletCard::id` returns the id of any
case.
Each returned card is an owned handle. Drop the cards you do not keep; dropping a card releases the
handle only, it does not revoke the card or remove it from the wallet.

A `PolymorphicPermissionCard` has the same operations and rules as a `PermissionCard`. Its `derive`
resolves the parent template for the agent and returns a `PersistentCard`. Its `install` resolves
the template for the target and installs the resolved card; the template stays in the wallet.

## Deriving cards

A derived card can only narrow authority. `Bounds` holds the four grant lists.

```rust
let grant = PatternGrant::new("card", "*", "*", "inspect", "*");

// From the wallet: the four lists are the full requested grants of the new card.
let card = permissions::derive_from_wallet(
    &Bounds { lower_positive: vec![grant.clone()], ..Bounds::default() },
    None,
)?;

// From a card: the positive lists select parent grants to retain,
// the negative lists add denials.
let child = card.derive(
    &Bounds { lower_positive: vec![grant], ..Bounds::default() },
    Some(&Lifespan::iso8601("PT1H")?),
)?;
```

Persistent derivation returns `PersistentCard::Concrete(PermissionCard)` or
`PersistentCard::Template(PolymorphicPermissionCard)`, labelled by the host. Match the case before
passing a card to a concrete or template schema. The sum supports the common card operations.

Persistent derivation accepts owner slot variables such as `?agent` and `?env`.
Slots in resource IDs are not supported and fail with `PermissionError::InvalidPattern`.
Child slots name the eventual installed holder, not the deriving caller.
A template parent is still resolved for the
caller in whose wallet it is installed. Every possible child instantiation must narrow that
resolved parent. The host stores the original slots; the universal envelope is proof only.
Explicit derivation inherits the parent's exact denials and adds the requested denials; wallet
derivation takes complete bounds. Slot-bearing denials cannot prove universal attenuation, so
the host may conservatively refuse a request; concrete inherited denials remain.
Scope cards stay concrete: requested slots fail with `PermissionError::InvalidPattern` and an
explicit message. For a slot-bearing tool owner, the host does not resolve aliases against caller
metadata and rejects metadata-dependent short flags. Use primary names.

A lifespan has the form `PnDTnHnMnS` (days, hours, minutes, seconds); `Lifespan::iso8601`
rejects years, months and weeks. `Lifespan::try_from(duration)` converts a positive
`std::time::Duration`. A child cannot outlive its parent.

## Inspecting cards

```rust
let view = card.inspect()?;          // the four grant lists, needs card:inspect
let id = card.id();                  // opaque CardId: equality and hashing only
let is_child = child.parents().contains(&id);
let expiry = child.expires_at();     // Option<Timestamp>
```

`id`, `parents` and `expires_at` need no verb, but the host traps (fails the invocation) if the
handle is unknown or its card is revoked or expired. `inspect`, `derive`, `derive_scope` and
`install` on a handle whose card is revoked or expired return `PermissionError::CardRevoked` or
`PermissionError::CardExpired`; use `inspect` to detect revocation.

## Lending authority for one call: scope cards

A `ScopeCard` lives only in memory and is valid only during the invocation that derived it. Lend
it to an awaited agent call; the callee holds the extra authority until that call ends. Lending
borrows the card and never consumes it.

The parents are cards in the agent's wallet (concrete or template) or scope cards of the current
invocation. A `&WalletCard` converts into a parent.

```rust
let wallet = permissions::wallet();
let scope: ScopeCard = permissions::derive_scope(&[(&wallet[0]).into()], &bounds)?;

let client = ReceiverClient::get("target".to_string());
client.with_scope_card(&scope).process(input).await;    // one call

let mut client = ReceiverClient::get("target".to_string());
client.set_default_scope_card(&scope);                  // every awaited call
client.process(input).await;
client.clear_default_scope_card();
```

The default only borrows the scope card. Keep the scope card alive while the client uses it as its
default; an awaited call after the scope card was dropped panics.

Only awaited calls carry a scope card, including the `pending_*` calls of `#[agent_client]`
clients. `trigger_*` and `schedule_*` calls never do.
The scope-card methods accept only a `ScopeCard`, never a `PermissionCard` or a
`PolymorphicPermissionCard`. When the invocation ends,
the host retires the scope card: the handle stays valid until it is dropped, but lending it (directly
or as a client's default) fails with a `denied` RPC error and using it as a `derive_scope` parent
fails with `PermissionError::NotPermitted`. The dynamic clients have `invoke_value_with_scope_card`,
`invoke_with_scope_card`, `pending_value_with_scope_card` and `pending_with_scope_card`.

A `ScopeCard` supports `id`, `parents` (its root cards), `expires_at` and `inspect`. `expires_at` is
the earliest expiry of its root cards, or `None` when no root card expires; the scope card stops
authorizing then. It cannot be revoked, installed, derived into a persistent card, serialized, or
passed as a method argument.

## Installing and revoking

Both consume the card, even when the host refuses the operation.

```rust
child.install(&Holder::Agent(target_agent_id))?;  // only agent targets are supported
let revoked: u32 = card.revoke()?;                // revokes the card and every card derived from it
```

`install` fails with `PermissionError::InstallRecipientMismatch` when the target does not match the
card's recipient patterns. Account and application targets are refused with
`PermissionError::NotPermitted`.

## Passing cards through agent and tool calls

`PermissionCard` and `PolymorphicPermissionCard` can be agent or tool method parameters or results.
Sending a card moves it to the receiver:

```rust
async fn forward(card: PermissionCard) -> PermissionCard {
    ReceiverClient::get("target".to_string()).accept(card).await
}
```

The schema of a `PermissionCard` declares `polymorphic: false`, and the schema of a
`PolymorphicPermissionCard` declares `polymorphic: true`. A received card is decoded into the type
that the parameter or result declares. The host refuses a method input or output whose card does
not match the declared kind.

## Rules

- A card is affine. After `revoke`, `install`, or sending it in a call, the card is moved, and
  every later use panics. Derive a separate card if you need to keep authority and send some.
- Cards cannot be constructed, cloned from data, serialized with serde, or persisted. Nothing turns
  a `CardId`, a UUID, or a grant list into a card.
- `CardId` supports only equality and hashing; printing it shows a redacted placeholder. It has no
  constructor and cannot be an agent method parameter or result. Compare ids within the agent, for
  example `permissions::wallet().into_iter().find(|card| card.id() == wanted)`. Ids grant nothing.
- `PermissionCard`, `PolymorphicPermissionCard` and `ScopeCard` are distinct types, and the SDK
  offers no conversion between them. A card value that goes through the schema-value codec takes
  the type that the decoder declares; the host keeps the real kind.
- `permissions::list_modules()` lists the resource classes and verbs the kernel knows, and
  `permissions::validate_grant(&grant)` checks a grant before you use it.
