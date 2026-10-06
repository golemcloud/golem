---
name: golem-permission-card-go
description: "Transfers opaque permission cards through Go agent and tool schemas. Use when delegated authority must cross an RPC or tool boundary in a Go Golem project."
---

# Permission cards in Go

Use `golem.PermissionCard` as a field of an agent method's or tool command's input, or as a result:

```go
type ForwardIn struct{ Card golem.PermissionCard }

var Forward = Agent.Method[ForwardIn, golem.PermissionCard]("forward")

var _ = agent.Handle(Forward, func(_ *golem.Context[state], in ForwardIn) golem.PermissionCard {
	return receiver.Accept.Call(receiver.Agent.Get(receiver.ID{Name: "target"}), receiver.AcceptIn{Card: in.Card})
})
```

In a tool, bind it like any other argument (`s.Positional(&a.Card)`) or return it.

- Cards are opaque, affine capabilities. Sending one — as an argument or a result — moves it: the copy that was sent is unusable afterwards (`golem.ErrPermissionCardMoved`), even if the call fails. Do not keep, log or send the same card twice.
- Use `golem.PolymorphicPermissionCard` only when the card's grants may leave the owner or resource id open.
- The SDK has no constructor for cards: they come from the host or from another call. This guide covers handing on a card you received.
- Generated guest clients spell these types the same way; external clients cannot carry cards.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | Passing a card to another agent |
| `golem-define-tool-go` | Taking or returning a card in a tool command |
