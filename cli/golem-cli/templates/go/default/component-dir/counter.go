// Counter agent in Go.
package main

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
		"increment", golem.Desc("Increase the count by one"))
	Add   = golem.DefineMethod[CounterID, AddIn, int64]("add", golem.Desc("Add to the count"))
	Value = golem.DefineMethod[CounterID, golem.Unit, int64]("value", golem.Desc("Return the current value"))
)

func init() {
	golem.Implement(Counter, Increment, func(ctx *golem.Context[CounterState], _ golem.Unit) (int64, error) {
		ctx.State.value++
		return ctx.State.value, nil
	})

	golem.Implement(Counter, Add, func(ctx *golem.Context[CounterState], in AddIn) (int64, error) {
		ctx.State.value += in.By
		return ctx.State.value, nil
	})

	// A handler can also be an ordinary Go method, bound with a method expression.
	golem.Implement(Counter, Value, golem.Bind0NoErr((*CounterState).current))
}

func (s *CounterState) current() int64 { return s.value }

// The SDK wires the component exports from its own init(); main stays empty.
func main() {}
