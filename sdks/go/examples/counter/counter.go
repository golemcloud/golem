// Package main is the canonical Golem Go agent example: a durable counter.
package main

import "github.com/golemcloud/golem/sdks/go/golem"

type CounterId struct{ Name string }    // constructor params; also the type-level marker
type CounterState struct{ count int64 } // private durable state
type AddIn struct{ By int64 }

var Counter = golem.DefineAgent[CounterId, CounterState](
	golem.Spec{Name: "CounterAgent", Description: "A durable counter", Mode: golem.Durable},
	func(id CounterId) *CounterState { return &CounterState{} },
)

var (
	Value     = golem.DefineMethod[CounterId, golem.Unit, int64]("value", golem.Desc("Return the current value"))
	Increment = golem.DefineMethod[CounterId, golem.Unit, int64]("increment")
	Add       = golem.DefineMethod[CounterId, AddIn, int64]("add", golem.Desc("Add to the counter"))
	Reset     = golem.DefineMethod[CounterId, golem.Unit, golem.Unit]("reset")
)

func init() {
	golem.Implement(Counter, Value, func(ctx *golem.Context[CounterState], _ golem.Unit) int64 {
		return ctx.State.count
	})
	golem.Implement(Counter, Increment, func(ctx *golem.Context[CounterState], _ golem.Unit) int64 {
		ctx.State.count++
		return ctx.State.count
	})
	golem.Implement(Counter, Add, func(ctx *golem.Context[CounterState], in AddIn) int64 {
		ctx.State.count += in.By
		return ctx.State.count
	})
	golem.Implement(Counter, Reset, func(ctx *golem.Context[CounterState], _ golem.Unit) golem.Unit {
		ctx.State.count = 0
		return golem.Unit{}
	})
}

// The SDK wires the component exports from its own init().
func main() {}
