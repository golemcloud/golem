// Package streaming is the DEFINITION of an agent showing agent streams: stream
// inputs and results, nested streams, recoverable items, a Durable Streams HTTP
// route, and reading and writing an external Durable Stream.
package streaming

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

// Config carries the bearer token of the external stream as an opaque secret.
type Config struct{ ExternalAuth golem.Secret[string] }

type SumIn struct{ Input golem.AgentStream[int64] }

type TransformIn struct {
	Prefix string
	Input  golem.AgentStream[int64]
}

type DurableEchoIn struct{ Input golem.AgentStream[string] }

type AppendExternalIn struct {
	Url        string
	ProducerId string
	Values     []string
	Close      bool
}

type ReadExternalIn struct{ Url string }

var Agent = golem.DefineConfiguredAgent[ID, Config](golem.Spec{
	Name: "StreamingAgent",
	HTTP: &golem.Mount{Path: "/durable-stream-agents/{name}"},
})

var (
	Sum         = Agent.Method[SumIn, int64]("sum", golem.Desc("Add up a stream of numbers"))
	Produce     = Agent.Method[golem.Unit, golem.AgentStream[int64]]("produce", golem.Desc("Stream a few numbers"))
	Transform   = Agent.Method[TransformIn, golem.AgentStream[string]]("transform", golem.Desc("Prefix every number of a stream"))
	Nested      = Agent.Method[golem.Unit, golem.AgentStream[golem.AgentStream[int64]]]("nested", golem.Desc("Stream streams"))
	Recoverable = Agent.Method[golem.Unit, golem.AgentStream[golem.Result[int64, string]]]("recoverable",
		golem.Desc("Stream items that may each fail"))
	Status = Agent.Method[golem.Unit, string]("status", golem.Desc("Report how many producers were cancelled"))
	// DurableEcho is served over the Durable Streams protocol: its input and its
	// result each get a URL that external clients append to, read and fork.
	DurableEcho = Agent.Method[DurableEchoIn, golem.AgentStream[string]]("durableEcho",
		golem.Desc("Echo a stream written over HTTP"),
		golem.HTTP(golem.PUT("/echo")))
	AppendExternal = Agent.Method[AppendExternalIn, golem.Option[string]]("appendExternal",
		golem.Desc("Append values to an external Durable Stream"))
	ReadExternal = Agent.Method[ReadExternalIn, []string]("readExternal",
		golem.Desc("Read every value of an external Durable Stream"))
)
