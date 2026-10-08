// Package streaming is the Go fixture driven by external_durable_streams.rs:
// an agent that writes to and reads from an external Durable Stream.
package streaming

import (
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durablestreams"
)

type ID struct{ Name string }

type Config struct{ ExternalAuth golem.Secret[string] }

var Agent = golem.DefineConfiguredAgent[ID, Config](golem.Spec{Name: "StreamingAgent"})

type AppendIn struct {
	Url        string
	ProducerId string
	Values     []string
	Close      bool
}

type ReadIn struct{ Url string }

var (
	AppendExternal = Agent.Method[AppendIn, golem.Option[string]]("appendExternal")
	ReadExternal   = Agent.Method[ReadIn, []string]("readExternal")
)

type state struct{}

func init() {
	agent := Agent.ImplementConfigured(func(*golem.InitContext[ID, state, Config]) *state { return &state{} })
	agent.Handle(AppendExternal, func(ctx *golem.Context[state], in AppendIn) golem.Option[string] {
		w, err := durablestreams.NewWriter(in.Url, "application/json", durablestreams.WriteOptions{
			ProducerID: golem.Some(in.ProducerId),
			Auth:       golem.Some(ctx.Config(Agent).ExternalAuth),
		})
		if err != nil {
			panic(err)
		}
		receipt, err := durablestreams.AppendJSON(w, in.Values, in.Close)
		if err != nil {
			panic(err)
		}
		return receipt.NextOffset
	})
	agent.Handle(ReadExternal, func(ctx *golem.Context[state], in ReadIn) []string {
		values, err := durablestreams.ReadJSON[string](in.Url, durablestreams.ReadOptions{
			Auth: golem.Some(ctx.Config(Agent).ExternalAuth),
		}).Collect()
		if err != nil {
			panic(err)
		}
		return values
	})
}
