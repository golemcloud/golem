// Package counter is a minimal durable agent for the Go SDK runtime test suite:
// a stateful counter whose value must survive replay/restart. It is the first
// guest that proves an agent-sdk-go component builds through the test-components
// harness and runs under the worker executor.
package counter

import "github.com/golemcloud/golem/sdks/go/golem"

// CounterID holds the constructor parameters and the type-level identity.
type CounterID struct{ Name string }

// CounterState is the agent's private, durable state.
type CounterState struct{ value int64 }

// AddIn is the parameter list for add.
type AddIn struct{ By int64 }

var Counter = golem.DefineAgent[CounterID, CounterState](
	golem.Spec{
		Name:        "CounterAgent",
		Description: "A durable counter for the Go SDK runtime tests",
		Mode:        golem.Durable,
	},
	func(id CounterID) *CounterState { return &CounterState{} },
)

var (
	Increment = golem.DefineMethod[CounterID, golem.Unit, int64](
		"increment", golem.Desc("Increase the count by one"))
	Add = golem.DefineMethod[CounterID, AddIn, int64](
		"add", golem.Desc("Add to the count"))
	Value = golem.DefineMethod[CounterID, golem.Unit, int64](
		"value", golem.Desc("Return the current value"))
)

func init() {
	golem.Implement(Counter, Increment, func(ctx *golem.Context[CounterState], _ golem.Unit) int64 {
		ctx.State.value++
		return ctx.State.value
	})
	golem.Implement(Counter, Add, func(ctx *golem.Context[CounterState], in AddIn) int64 {
		ctx.State.value += in.By
		return ctx.State.value
	})
	golem.Implement(Counter, Value, golem.Bind0((*CounterState).current))
}

func (s *CounterState) current() int64 { return s.value }
