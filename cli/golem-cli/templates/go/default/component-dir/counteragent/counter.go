// Package counteragent is the DEFINITION of the counter agent: its identity,
// method descriptors, and input/output types. Other agents import this package
// to call the counter; the behaviour and state live in counteragentimpl.
package counteragent

import "github.com/golemcloud/golem/sdks/go/golem"

// CounterID holds the agent's constructor parameters, and doubles as the
// type-level identity used by typed cross-agent calls.
type CounterID struct{ Name string }

// AddIn is a method's parameter list: one WIT parameter per exported field.
type AddIn struct{ By int64 }

// Agent is the state-free definition. Mode defaults to Durable.
var Agent = golem.DefineAgent[CounterID](golem.Spec{
	Name:        "CounterAgent",
	Description: "A simple durable counter",
})

// Method descriptors are package-level vars on purpose: the same value drives the
// published schema, the implementation, and calls from other agents.
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
