// Package session is the DEFINITION of the session agent. Its behaviour, private
// state, and custom snapshot serialization live in the impl subpackage.
package session

import "github.com/golemcloud/golem/sdks/go/golem"

// ID identifies the session agent by its constructor parameters.
type ID struct{ User string }

// SpendIn is the parameter list for the spend method.
type SpendIn struct{ Amount int64 }

// Agent snapshots its state every 5 invocations instead of the default cadence.
// Mode defaults to Durable.
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "SessionAgent",
	Description: "A session that snapshots its running total every few invocations",
	Snapshot:    golem.SnapshotEveryN(5),
})

var (
	Spend = golem.DefineMethod[ID, SpendIn, int64]("spend", golem.Desc("Add to the running total"))
	Total = golem.DefineMethod[ID, golem.Unit, int64]("total", golem.Desc("Return the running total"))
)
