// Package divergence is the DEFINITION of the failed-update reader test agent. It
// reads the divergences of the failed updates in its own oplog through both oplog
// readers, so a test can check that a Go guest sees the requests of both host
// calls of a divergence. Behaviour lives in divergence/impl.
package divergence

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

// Call is a host call step of a divergence.
type Call struct {
	FunctionName   string
	CarriesRequest bool
	// The request as the guest sees it, rendered with fmt; nil when the host call has no
	// request.
	Request *string
}

// Step is one side of a divergence. Kind is the replay-step case tag: 0 host call,
// 1 invocation end, 2 end of history, 3 other.
type Step struct {
	Kind uint8
	Call *Call
}

// Divergence is the divergence of one failed update, with the index of the
// FailedUpdate entry that holds it.
type Divergence struct {
	EntryIndex   uint64
	FailureClass uint8
	OplogIndex   uint64
	Expected     Step
	Actual       Step
	Mismatched   []uint8
	// Whether the requests of the two host calls are different values.
	RequestsDiffer bool
	// The case tag of the attribution of the FailedUpdate entry: 0 the agent
	// itself, 1 an entity inside it.
	Attribution uint8
}

// Report holds the divergences that each oplog reader returned.
type Report struct {
	FromGet    []Divergence
	FromSearch []Divergence
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "DivergenceReaderAgent", Description: "Reads the divergences of failed updates from its own oplog", Mode: golem.Durable,
})

var (
	Read = Agent.Method[golem.Unit, Report]("read", golem.Description("Read the divergences of the failed updates in the own oplog through Get and Search"))
)
