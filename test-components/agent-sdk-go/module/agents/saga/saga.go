// Package saga is the DEFINITION of the agent exercising the SDK's transaction
// (saga) helpers. Behaviour lives in saga/impl.
package saga

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type RunIn struct {
	// Fail makes the second step return an error, which rolls the transaction
	// back and runs the first step's compensation.
	Fail bool
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "SagaAgent", Description: "Exercises the Go SDK transaction (saga) helpers", Mode: golem.Durable,
})

var (
	// Run executes a two-step transaction, recording each step and compensation
	// in state. It returns "committed" or "rolled-back".
	Run = golem.DefineMethod[Id, RunIn, string]("run",
		golem.Desc("Run a two-step saga; roll back when Fail is set"))
	// Log returns the recorded step/compensation names in order.
	Log = golem.DefineMethod[Id, golem.Unit, []string]("log",
		golem.Desc("Return the recorded step and compensation names"))
)
