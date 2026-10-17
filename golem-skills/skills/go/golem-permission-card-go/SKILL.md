---
name: golem-permission-card-go
description: "Passes, derives, inspects, installs and revokes permission cards, and lends scope cards to agent calls, in a Go Golem project. Use when delegated authority must cross an RPC or tool boundary, or when an agent manages the cards in its wallet."
---

# Permission cards in Go

A permission card is delegated authority. An agent holds cards in its wallet. The `golem` package lets it hand cards on, derive narrower cards, inspect them, install them on other agents, revoke them, and lend a scope card to an awaited call.

## Passing a card

Use `golem.PermissionCard` as a field of an agent method's or tool command's input, or as a result:

```go
type ForwardIn struct{ Card golem.PermissionCard }

var Forward = Agent.Method[ForwardIn, golem.PermissionCard]("forward")

var _ = agent.Handle(Forward, func(_ *golem.Context[state], in ForwardIn) golem.PermissionCard {
	return receiver.Accept.MustCall(receiver.Agent.Get(receiver.ID{Name: "target"}), receiver.AcceptIn{Card: in.Card})
})
```

In a tool, bind it like any other argument (`s.Positional(&a.Card)`) or return it. Use `golem.PolymorphicPermissionCard` when the card's grants may leave the owner or resource id open (a template card). The card types do not convert into each other. A receiver decodes a card as the type its parameter declares. The host refuses an agent call input or output whose card does not match the declared kind.

## Managing cards

What an agent may do with cards is granted by cards: the `card:derive`, `card:inspect`, `card:install` and `card:revoke` verbs. A refused call returns a `*golem.PermissionError`; its `Kind` is the host's case (`golem.PermissionNotPermitted`, `golem.PermissionInstallRecipientMismatch`, …).

```go
grant := golem.PatternGrant{Class: "kv", Owner: "*", Recipient: "*", Verb: "read", ResourceID: "orders.*"}

// Derive from whichever wallet card covers the request.
card, err := golem.DeriveFromWallet(golem.Bounds{LowerPositive: []golem.PatternGrant{grant}}, golem.None[golem.Lifespan]())
if err != nil {
	var pe *golem.PermissionError
	if errors.As(err, &pe) && pe.Kind == golem.PermissionNotPermitted { … }
}

// Inspect it, and derive a child that expires in an hour.
view := card.MustInspect() // view.LowerPositive, LowerNegative, UpperPositive, UpperNegative
child := card.MustDerive(golem.Bounds{LowerPositive: view.LowerPositive}, golem.Some(golem.LifespanOf(time.Hour)))

// Install the child on another agent, then revoke the card and everything derived from it.
err = child.Install(golem.AgentHolder(golem.AgentID{ComponentID: componentID, AgentID: client.AgentID()}))
n, err := card.Revoke() // n counts the revoked cards
```

- `golem.Bounds` holds the four grant lists. For `card.Derive`, the positive lists name the parent's grants the child keeps and the negative lists the denials it adds. For `golem.DeriveFromWallet` and `golem.DeriveScope` the four lists are the child's complete lattice. The host rejects a lattice the parents do not cover.
- Requested grants must be concrete. A grant with a slot variable such as `?agent` or `?env`, for example one that `Inspect` returned for a template wallet card, fails with `golem.PermissionInvalidPattern`: deriving a new template card, with slot variables in its grants, is not supported yet. A child derived from a template parent is a concrete `PermissionCard`.
- `golem.LifespanOf(d)` takes a positive `time.Duration`. `golem.ParseLifespan("P1DT12H")` takes ISO-8601 text of days, hours, minutes and seconds (`PnDTnHnMnS`). Years, months and weeks have no fixed length, and the host rejects them. A zero lifespan is refused.
- `golem.Wallet()` returns the wallet's cards as `golem.WalletCard` values, each labelled with its kind. `w.Kind()` is `golem.WalletCardConcrete`, `golem.WalletCardTemplate` or `golem.WalletCardScope`, and exactly one accessor gives the card as the type of its kind: `w.Concrete()` a `golem.PermissionCard`, `w.Template()` a `golem.PolymorphicPermissionCard` (its grants may contain slot variables such as `?agent`), `w.Scope()` a `golem.ScopeCard`. The other accessors return `false`, and the card types do not convert into each other. `ID`, `Parents`, `ExpiresAt` and `Inspect` work on a `WalletCard` of every kind, and it can be a `DeriveScope` parent. A `WalletCard` cannot be sent; send the `PermissionCard` from `Concrete()` or the `PolymorphicPermissionCard` from `Template()`. Each is a fresh handle: call `Release()` on the ones you do not keep, or `golem.ReleaseCards(cards...)`.
- During an invocation that received a scope card, the list also contains that scope card as a `WalletCardScope` entry if the agent holds `card:inspect`. Like a scope card from `DeriveScope`, it is retired when the invocation ends.

```go
for _, w := range golem.Wallet() {
    switch w.Kind() {
    case golem.WalletCardConcrete:
        card, _ := w.Concrete() // golem.PermissionCard: send, install, revoke or derive
        _ = card
    case golem.WalletCardTemplate:
        card, _ := w.Template() // golem.PolymorphicPermissionCard
        _ = card
    case golem.WalletCardScope:
        scope, _ := w.Scope() // golem.ScopeCard: attach to awaited calls
        _ = scope
    }
}
```

- Deriving from a template card resolves it for the calling agent; the card must be in the agent's wallet.
- `golem.AgentHolder` is the only install target the host accepts today; `golem.AccountHolder` and `golem.AppHolder` return `PermissionNotPermitted`.
- `card.ID()` returns an opaque `golem.CardID`: compare it with `card.Parents()` or use it as a map key. It has no other use, and nothing turns it back into a card. Printing or serializing it shows a placeholder, never the identifier. `ID`, `Parents` and `ExpiresAt` trap when the host no longer knows the card.
- `Revoke` takes the authority of the card and of every card derived from it away from their holders.
- `golem.ListKernelModules()` lists the resource classes and verbs; `golem.ValidateGrant(g)` checks one grant.

## Lending a scope card to a call

`golem.DeriveScope` returns a `golem.ScopeCard`. Attached to an awaited call, it lends the callee its grants for that invocation only:

```go
scope := golem.MustDeriveScope(golem.Bounds{LowerPositive: []golem.PatternGrant{grant}}, parent)
defer scope.Release()

// One call:
out, err := Lookup.WithScopeCard(scope).Call(client, LookupIn{Key: "k"})

// Every awaited call through a copy of the client:
scoped := client.WithScopeCard(scope)
out, err = Lookup.Call(scoped, LookupIn{Key: "k"})
```

- A scope card is valid only during the invocation that derived it. Do not keep it, or a client that carries it, in the agent's state for a later invocation: when the invocation ends the host retires it, denies the awaited calls that carry it with a `*golem.RemoteCallError` of kind `golem.RemoteDenied`, and refuses it as a `DeriveScope` parent with `golem.PermissionNotPermitted`. `Release()` on a retired scope card is safe.
- Only `Call`, `MustCall` and `CallAsync` carry a scope card; `Trigger` and `Schedule` never do. A card attached to the call takes precedence over the client's default.
- Only a `golem.ScopeCard` attaches: passing a `PermissionCard` or a `WalletCard` does not compile. A `ScopeCard` has `ID`, `Parents`, `ExpiresAt`, `Inspect` and `Release`. `ExpiresAt` returns the earliest expiry of the scope card's root cards, or `None` when no root card expires; the scope card stops authorizing then. A callee that lists the lent scope card in its wallet sees the same expiry. It cannot be sent, installed, revoked or derived from with `Derive`; a method or tool input that holds one is a definition error. It can be a parent of another `DeriveScope`.
- Attaching a card lends it: the caller keeps it. A released scope card panics with `golem.ErrPermissionCardMoved` before the call encodes its input, so no input card moves.
- A generated guest client has the same `WithScopeCard` method. The `reflection` clients have `WithScopeCard` too, and take a per-call card as `reflection.CallScopeCard(scope)`.

## Moves

A card is affine. Sending it as an argument or result, `Revoke` and `Install` move it — even when the call fails — and every later use panics with `golem.ErrPermissionCardMoved`. Do not keep, log or send the same card twice. A card cannot be built from data or serialized: `json.Marshal` of a card fails. A `golem.TypedValue` that holds a card is sent once too. Decoding it with `golem.DecodeTypedValue` takes the card out, and so does sending or returning it: handing an input on with a universal tool middleware's `Next` or `Start`, sending it with a dynamic tool client, or returning it as a middleware result. After that, decoding or sending the same value fails with `golem.ErrPermissionCardTaken`, and a middleware result or passed-on tool error that holds such a value fails as an invalid result. Build the value you pass on from the decoded one instead. A tool error's `Match` decodes its payload once and returns the same card on every match.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | Passing a card to another agent, or lending a scope card |
| `golem-define-tool-go` | Taking or returning a card in a tool command |
