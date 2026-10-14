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

In a tool, bind it like any other argument (`s.Positional(&a.Card)`) or return it. Use `golem.PolymorphicPermissionCard` when the card's grants may leave the owner or resource id open (a template card).

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
- `golem.LifespanOf(d)` takes a `time.Duration`; `golem.ParseLifespan("P30D")` takes ISO-8601 text, including calendar durations a `time.Duration` cannot express.
- `golem.Wallet()` returns the wallet's cards as `golem.PolymorphicPermissionCard` (a wallet card may be a template). Each is a fresh handle: call `Release()` on the ones you do not keep, or `golem.ReleaseCards(cards...)`.
- Deriving from a template card resolves it for the calling agent; the card must be in the agent's wallet.
- `golem.AgentHolder` is the only install target the host accepts today; `golem.AccountHolder` and `golem.AppHolder` return `PermissionNotPermitted`.
- `card.ID()` returns an opaque `golem.CardID`: compare it with `card.Parents()` or use it as a map key. It has no other use, and nothing turns it back into a card. `ID`, `Parents` and `ExpiresAt` trap when the host no longer knows the card.
- `golem.ListKernelModules()` lists the resource classes and verbs; `golem.ValidateGrant(g)` checks one grant.

## Lending a scope card to a call

A scope card lives for the current invocation and is never stored. Attached to an awaited call, it lends the callee its grants for that invocation only:

```go
scope := golem.MustDeriveScope(golem.Bounds{LowerPositive: []golem.PatternGrant{grant}}, parent)

// One call:
out, err := Lookup.WithScopeCard(scope).Call(client, LookupIn{Key: "k"})

// Every awaited call through a client:
scoped := client.WithScopeCard(scope)
out, err = Lookup.Call(scoped, LookupIn{Key: "k"})
```

Only `Call`, `MustCall` and `CallAsync` carry a scope card; `Trigger` and `Schedule` never do. A generated guest client has the same `WithScopeCard` method. Attaching a card lends it: the caller keeps it.

## Moves

A card is affine. Sending it as an argument or result, `Revoke` and `Install` move it — even when the call fails — and every later use panics with `golem.ErrPermissionCardMoved`. Do not keep, log or send the same card twice. A card cannot be built from data or serialized: `json.Marshal` of a card fails.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | Passing a card to another agent, or lending a scope card |
| `golem-define-tool-go` | Taking or returning a card in a tool command |
