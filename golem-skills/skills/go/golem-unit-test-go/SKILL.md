---
name: golem-unit-test-go
description: "Unit tests a Go Golem component natively with go test, without building or deploying. Use when adding tests to a Go agent or tool, checking agent and tool definitions for mistakes, testing agent streams, or round-tripping values through Golem's schema."
---

# Unit Testing a Go Golem Component

A Go component's packages build natively as well as for WebAssembly, so `go test ./...` runs
ordinary Go tests against them on the development machine. There is no Golem host in a native
test: test the definitions, the logic and the values, and leave what talks to Golem (RPC,
durability, key-value, blobstore, agent management, tool calls) to the integration tests (see
`golem-integration-test-setup`).

```shell
go test ./...
```

## Check the definitions

Mistakes in agent and tool declarations — an unsupported type, a bad HTTP route, an unbound tool
argument, a method handled twice — are reported by `golem.DefinitionErrors()`. Asserting it is
empty catches them before `golem build`:

```go
package counter

import (
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
)

func TestDefinitionsAreValid(t *testing.T) {
	for _, err := range golem.DefinitionErrors() {
		t.Error(err)
	}
}
```

The check covers what the test binary registers, so it runs in the package that implements the
agents (or one that imports it). An agent's methods are the ones it handles: a method descriptor
without a handler is not part of the agent and is not checked.

## Test the logic

Handlers are closures registered in `init`; keep their logic in plain functions or methods on the
state, and test those directly:

```go
type State struct{ Total int64 }

func (s *State) add(n int64) int64 {
	s.Total += max(n, 0)
	return s.Total
}

func init() {
	impl := Agent.Implement(func(ID) *State { return &State{} })
	impl.Handle(Add, func(ctx *golem.Context[State], in AddIn) int64 { return ctx.State.add(in.N) })
}
```

```go
func TestAdd(t *testing.T) {
	s := &State{}
	if got := s.add(3); got != 3 {
		t.Fatalf("add(3) = %d", got)
	}
}
```

## Test streams

`golem.AgentStream` runs natively as an in-memory pipe, so a producer and its consumer work in a
test as they do between agents:

```go
func countUp(total int64) golem.AgentStream[string] {
	return golem.ProduceStream(func(w *golem.AgentStreamWriter[string]) error {
		for i := int64(1); i <= total; i++ {
			if err := w.Write(fmt.Sprint(i)); err != nil {
				return err
			}
		}
		return nil
	})
}

func TestCountUp(t *testing.T) {
	got, err := countUp(3).Collect()
	if err != nil || len(got) != 3 || got[2] != "3" {
		t.Fatalf("stream gave %v, %v", got, err)
	}
}
```

`golem.StreamOf(items...)` makes a stream of fixed values to feed code that consumes one.

## Round-trip values

`golem.EncodeTypedValue` and `golem.DecodeTypedValue` convert through the same schema a method's
parameters, results and snapshots use, which shows how a type travels:

```go
func TestStateRoundTrips(t *testing.T) {
	v, err := golem.EncodeTypedValue(State{Total: 7})
	if err != nil {
		t.Fatal(err)
	}
	back, err := golem.DecodeTypedValue[State](v)
	if err != nil || back.Total != 7 {
		t.Fatalf("round trip gave %+v, %v", back, err)
	}
	t.Log(v.JSON())
}
```

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Declaring agents, methods and their types |
| `golem-define-tool-go` | Declaring tools, whose mistakes `DefinitionErrors` also reports |
| `golem-integration-test-setup` | Testing against a running Golem server |
| `golem-test-crash-recovery` | Testing durable behavior across crashes and restarts |
