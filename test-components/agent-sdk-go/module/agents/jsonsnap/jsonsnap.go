// Package jsonsnap is the DEFINITION of the agent exercising the SDK's default
// snapshot: its state has exported fields and no Snapshotter, so the SDK saves
// and restores it as JSON. Behaviour lives in jsonsnap/impl.
package jsonsnap

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type AddIn struct{ Item string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name:        "JSONSnapAgent",
	Description: "Exercises the Go SDK default JSON snapshot",
	Mode:        golem.Durable,
	Snapshot:    golem.SnapshotEveryN(2),
})

var (
	Add    = Agent.Method[AddIn, int64]("add", golem.Description("Append an item and return the item count"))
	Items  = Agent.Method[golem.Unit, []string]("items", golem.Description("Return the items in order"))
	Origin = Agent.Method[golem.Unit, string]("origin", golem.Description("Return how the running state was built"))
)
