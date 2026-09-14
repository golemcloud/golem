// Package snapstate is the DEFINITION of the agent exercising the SDK's custom
// snapshot support (golem.Snapshotter). Behaviour lives in snapstate/impl.
package snapstate

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name:        "SnapAgent",
	Description: "Exercises the Go SDK custom snapshot (Snapshotter)",
	Mode:        golem.Durable,
	Snapshot: golem.SnapshotEveryN(2),
})

var (
	Bump = golem.DefineMethod[Id, golem.Unit, int64]("bump",
		golem.Desc("Increase the counter and return it"))
	Value = golem.DefineMethod[Id, golem.Unit, int64]("value",
		golem.Desc("Return the current counter"))
)
