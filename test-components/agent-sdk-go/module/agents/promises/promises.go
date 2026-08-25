// Package promises is the DEFINITION of the agent exercising the SDK's promise
// wrapper (golem.NewPromise / Await / CompletePromise). Behaviour lives in
// promises/impl.
package promises

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type OplogIdxIn struct{ OplogIdx int64 }

type CompleteIn struct {
	OplogIdx int64
	Value    string
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "PromiseAgent", Description: "Exercises the Go SDK promise wrapper", Mode: golem.Durable,
})

var (
	// Create makes a promise and returns its oplog index — the part of the
	// PromiseID a completer needs alongside this agent's own id.
	Create = golem.DefineMethod[Id, golem.Unit, int64]("create",
		golem.Desc("Create a promise and return its oplog index"))
	// Await suspends until the promise is completed and returns its payload.
	Await = golem.DefineMethod[Id, OplogIdxIn, string]("await",
		golem.Desc("Await the promise with the given oplog index"))
	// Complete completes a promise from inside the agent (the agent-to-agent path),
	// reporting whether this call was the one that completed it.
	Complete = golem.DefineMethod[Id, CompleteIn, bool]("complete",
		golem.Desc("Complete the promise with the given oplog index"))
)
