// Package impl is the IMPLEMENTATION of the streaming agent. Importing it
// registers the agent.
package impl

import (
	"fmt"

	"component-name/agents/streaming"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durablestreams"
)

type state struct{ cancelledProducers int }

// stream produces values; a write fails once the consumer stops reading, which
// is how a producer learns it was cancelled.
func stream[T any](s *state, values ...T) golem.AgentStream[T] {
	return golem.ProduceStream(func(w *golem.AgentStreamWriter[T]) error {
		for _, v := range values {
			if err := w.Write(v); err != nil {
				s.cancelledProducers++
				return nil
			}
		}
		return nil
	})
}

var agent = streaming.Agent.ImplementConfigured(func(*golem.InitContext[streaming.ID, state, streaming.Config]) *state {
	return &state{}
})

func init() {
	agent.Handle(streaming.Sum, func(_ *golem.Context[state], in streaming.SumIn) int64 {
		var total int64
		for v := range in.Input.All() {
			total += v
		}
		return total
	})
	agent.Handle(streaming.Produce, func(ctx *golem.Context[state], _ golem.Unit) golem.AgentStream[int64] {
		return stream[int64](ctx.State, 1, 2, 3)
	})
	agent.Handle(streaming.Transform, func(ctx *golem.Context[state], in streaming.TransformIn) golem.AgentStream[string] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[string]) error {
			for v := range in.Input.All() {
				if err := w.Write(fmt.Sprintf("%s:%d", in.Prefix, v)); err != nil {
					// Stopping early also closes the input, propagating the
					// cancellation upstream.
					ctx.State.cancelledProducers++
					return nil
				}
			}
			return nil
		})
	})
	agent.Handle(streaming.Nested, func(ctx *golem.Context[state], _ golem.Unit) golem.AgentStream[golem.AgentStream[int64]] {
		return stream(ctx.State, stream[int64](ctx.State, 10, 20), stream[int64](ctx.State, 30, 40))
	})
	agent.Handle(streaming.Recoverable, func(ctx *golem.Context[state], _ golem.Unit) golem.AgentStream[golem.Result[int64, string]] {
		return stream(ctx.State,
			golem.Ok[int64, string](1),
			golem.Err[int64]("this item could not be produced"),
			golem.Ok[int64, string](2))
	})
	agent.Handle(streaming.Status, func(ctx *golem.Context[state], _ golem.Unit) string {
		return fmt.Sprintf("ready (%d cancelled producers)", ctx.State.cancelledProducers)
	})
	agent.Handle(streaming.DurableEcho, func(_ *golem.Context[state], in streaming.DurableEchoIn) golem.AgentStream[string] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[string]) error {
			for v := range in.Input.All() {
				if err := w.Write("echo:" + v); err != nil {
					return nil
				}
			}
			return nil
		})
	})
	agent.Handle(streaming.AppendExternal, func(ctx *golem.Context[state], in streaming.AppendExternalIn) golem.Option[string] {
		w := golem.Must(durablestreams.NewWriter(in.Url, "application/json", durablestreams.WriteOptions{
			ProducerID: golem.Some(in.ProducerId),
			Auth:       golem.Some(ctx.Config(streaming.Agent).ExternalAuth),
		}))
		return golem.Must(durablestreams.AppendJSON(w, in.Values, in.Close)).NextOffset
	})
	agent.Handle(streaming.ReadExternal, func(ctx *golem.Context[state], in streaming.ReadExternalIn) []string {
		return golem.Must(durablestreams.ReadJSON[string](in.Url, durablestreams.ReadOptions{
			Auth: golem.Some(ctx.Config(streaming.Agent).ExternalAuth),
		}).Collect())
	})
}
