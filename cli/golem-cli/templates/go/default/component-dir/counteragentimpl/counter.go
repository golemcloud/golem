// Package counteragentimpl is the IMPLEMENTATION of the counter agent: its private
// state and method handlers. main.go blank-imports this package, and importing it
// registers the agent with the SDK.
package counteragentimpl

import (
	"component-name/counteragent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// state is the agent's private, durable state — invisible to callers.
type state struct{ value int64 }

func (s *state) current() int64 { return s.value }

// Implement bundles the constructor and every method handler in one call.
// Handlers return only their output value; signal failure by panicking (the SDK
// recovers it into an agent-error). Model expected outcomes as a golem.Result.
var _ = golem.Implement(counteragent.Agent,
	func(counteragent.CounterID) *state { return &state{} },
	golem.Bound(counteragent.Increment, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.value++
		return ctx.State.value
	}),
	golem.Bound(counteragent.Add, func(ctx *golem.Context[state], in counteragent.AddIn) int64 {
		ctx.State.value += in.By
		return ctx.State.value
	}),
	// A handler can also be an ordinary Go method, bound with a method expression.
	golem.Bound(counteragent.Value, golem.Bind0((*state).current)),
)
