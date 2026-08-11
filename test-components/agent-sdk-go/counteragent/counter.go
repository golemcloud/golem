// Package counteragent is the DEFINITION of the durable counter agent: its
// identity, method descriptors, and input/output types. Other agents import this
// package to call the counter; the behaviour and state live in counteragentimpl.
package counteragent

import "github.com/golemcloud/golem/sdks/go/golem"

// CounterID holds the constructor parameters and the type-level identity.
type CounterID struct{ Name string }

// AddIn is the parameter list for add.
type AddIn struct{ By int64 }

var Agent = golem.DefineAgent[CounterID](golem.Spec{
	Name:        "CounterAgent",
	Description: "A durable counter for the Go SDK runtime tests",
	Mode:        golem.Durable,
})

var (
	Increment = golem.DefineMethod[CounterID, golem.Unit, int64](
		"increment", golem.Desc("Increase the count by one"))
	Add = golem.DefineMethod[CounterID, AddIn, int64](
		"add", golem.Desc("Add to the count"))
	Value = golem.DefineMethod[CounterID, golem.Unit, int64](
		"value", golem.Desc("Return the current value"))
)
