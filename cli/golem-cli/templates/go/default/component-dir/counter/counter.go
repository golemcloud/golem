// Counter agent in Go. Each agent lives in its own package; main.go blank-imports
// this package so its init() registers the agent with the SDK.
package counter

import "github.com/golemcloud/golem/sdks/go/golem"

// CounterID holds the agent's constructor parameters, and doubles as the
// type-level identity used by typed cross-agent calls.
type CounterID struct{ Name string }

// CounterState is the agent's private, durable state.
type CounterState struct{ value int64 }

// AddIn is a method's parameter list: one WIT parameter per exported field.
type AddIn struct{ By int64 }

var Counter = golem.DefineAgent[CounterID, CounterState](
	golem.Spec{
		Name:        "CounterAgent",
		Description: "A simple durable counter",
		Mode:        golem.Durable,
	},
	func(id CounterID) *CounterState { return &CounterState{} },
)

// Method descriptors are package-level vars on purpose: the same value drives
// the published schema, the implementation below, and calls from other agents.
var (
	Increment = golem.DefineMethod[CounterID, golem.Unit, int64](
		"increment",
		golem.Desc("Increase the count by one"),
	)
	Add = golem.DefineMethod[CounterID, AddIn, int64](
		"add",
		golem.Desc("Add to the count"),
	)
	Value = golem.DefineMethod[CounterID, golem.Unit, int64](
		"value",
		golem.Desc("Return the current value"),
	)
)

func init() {
	// Handlers return only their output value; signal failure by panicking (the
	// SDK recovers it into an agent-error). Model expected outcomes as a Result.
	golem.Implement(Counter, Increment, func(ctx *golem.Context[CounterState], _ golem.Unit) int64 {
		ctx.State.value++
		return ctx.State.value
	})

	golem.Implement(Counter, Add, func(ctx *golem.Context[CounterState], in AddIn) int64 {
		ctx.State.value += in.By
		return ctx.State.value
	})

	// A handler can also be an ordinary Go method, bound with a method expression.
	golem.Implement(Counter, Value, golem.Bind0((*CounterState).current))
}

func (s *CounterState) current() int64 { return s.value }
