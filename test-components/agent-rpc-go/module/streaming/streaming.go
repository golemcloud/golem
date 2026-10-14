// Package streaming is the Go variant of the cross-language streaming RPC
// fixture: GoStreamingRpcCaller calls GoStreamingRpcCrossTarget, usually on
// another executor, through every streaming shape a method can have.
package streaming

import (
	"errors"
	"fmt"
	"sync"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type NestedStreamInput struct {
	Labels golem.AgentStream[string]
	Values golem.AgentStream[uint32]
}

type NestedStreamItem struct {
	Label  string
	Values golem.AgentStream[uint32]
}

type GoStreamingRpcReport struct {
	Consumed         []uint32
	Produced         []uint32
	Transformed      []uint32
	ScalarLabel      string
	ScalarValues     []uint32
	NestedLabels     []string
	NestedValues     []uint32
	NestedItemLabels []string
	NestedItemValues [][]uint32
	FirstSibling     []uint32
	SecondSibling    []uint32
	Forwarded        []uint32
	Recoverable      []golem.Result[uint32, string]
	EarlyValue       uint32
	Ping             string
	Increment        uint64
}

type (
	TargetID struct{ Name string }
	CallerID struct{ TargetName string }
)

type StreamIn struct{ Input golem.AgentStream[uint32] }

type NestedIn struct{ Input NestedStreamInput }

var Target = golem.DefineAgent[TargetID](golem.Spec{Name: "GoStreamingRpcCrossTarget"})

var (
	consume                   = Target.Method[StreamIn, []uint32]("consume")
	consumeOneThenDrop        = Target.Method[StreamIn, uint32]("consume_one_then_drop")
	produce                   = Target.Method[golem.Unit, golem.AgentStream[uint32]]("produce")
	transform                 = Target.Method[StreamIn, golem.AgentStream[uint32]]("transform")
	scalarAndStream           = Target.Method[golem.Unit, golem.Tuple2[string, golem.AgentStream[uint32]]]("scalar_and_stream")
	consumeNested             = Target.Method[NestedIn, golem.Tuple2[[]string, []uint32]]("consume_nested")
	produceNestedItems        = Target.Method[golem.Unit, golem.AgentStream[NestedStreamItem]]("produce_nested_items")
	produceSiblings           = Target.Method[golem.Unit, golem.Tuple2[golem.AgentStream[uint32], golem.AgentStream[uint32]]]("produce_siblings")
	forwardUnread             = Target.Method[StreamIn, golem.AgentStream[uint32]]("forward_unread")
	produceRecoverableResults = Target.Method[golem.Unit, golem.AgentStream[golem.Result[uint32, string]]]("produce_recoverable_results")
	produceFailure            = Target.Method[golem.Unit, golem.AgentStream[uint32]]("produce_failure")
	produceCancellable        = Target.Method[golem.Unit, golem.AgentStream[uint32]]("produce_cancellable")
	produceMany               = Target.Method[golem.Unit, golem.AgentStream[uint32]]("produce_many")
	ping                      = Target.Method[golem.Unit, string]("ping")
	increment                 = Target.Method[golem.Unit, uint64]("increment")
)

var Caller = golem.DefineAgent[CallerID](golem.Spec{Name: "GoStreamingRpcCaller"})

var (
	run                      = Caller.Method[golem.Unit, GoStreamingRpcReport]("run")
	callProducerFailure      = Caller.Method[golem.Unit, []uint32]("call_producer_failure")
	callProducerCancellation = Caller.Method[golem.Unit, golem.Tuple2[uint32, string]]("call_producer_cancellation")
	callServerCancellation   = Caller.Method[golem.Unit, golem.Tuple3[uint32, bool, string]]("call_server_cancellation")
	dropOutputEarly          = Caller.Method[golem.Unit, golem.Tuple2[uint32, string]]("drop_output_early")
	callScalar               = Caller.Method[golem.Unit, uint64]("call_scalar")
)

func uintRange(start, count uint32) []uint32 {
	values := make([]uint32, count)
	for i := range values {
		values[i] = start + uint32(i)
	}
	return values
}

// both collects two streams at once, as a reader of sibling streams must: one
// of them can block until the other is drained.
func both[A, B any](a golem.AgentStream[A], b golem.AgentStream[B]) ([]A, []B) {
	var wg sync.WaitGroup
	var first []A
	wg.Go(func() { first = a.Collect() })
	second := b.Collect()
	wg.Wait()
	return first, second
}

type targetState struct {
	name  string
	count uint64
}

type callerState struct{ targetName string }

func init() {
	target := Target.Implement(func(id TargetID) *targetState { return &targetState{name: id.Name} })
	target.Handle(consume, func(_ *golem.Context[targetState], in StreamIn) []uint32 {
		return in.Input.Collect()
	})
	target.Handle(consumeOneThenDrop, func(_ *golem.Context[targetState], in StreamIn) uint32 {
		defer in.Input.Close()
		first, ok := in.Input.Next()
		if !ok {
			panic("the input ended before its first value")
		}
		return first
	})
	target.Handle(produce, func(*golem.Context[targetState], golem.Unit) golem.AgentStream[uint32] {
		return golem.StreamOf[uint32](3, 5, 8)
	})
	target.Handle(transform, func(_ *golem.Context[targetState], in StreamIn) golem.AgentStream[uint32] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[uint32]) error {
			defer in.Input.Close()
			for v := range in.Input.All() {
				if err := w.Write(v + 100); errors.Is(err, golem.ErrReaderGone) {
					return nil
				}
			}
			return nil
		})
	})
	target.Handle(scalarAndStream, func(*golem.Context[targetState], golem.Unit) golem.Tuple2[string, golem.AgentStream[uint32]] {
		return golem.Tuple2[string, golem.AgentStream[uint32]]{A: "go", B: golem.StreamOf[uint32](13, 21, 34)}
	})
	target.Handle(consumeNested, func(_ *golem.Context[targetState], in NestedIn) golem.Tuple2[[]string, []uint32] {
		labels, values := both(in.Input.Labels, in.Input.Values)
		return golem.Tuple2[[]string, []uint32]{A: labels, B: values}
	})
	target.Handle(produceNestedItems, func(*golem.Context[targetState], golem.Unit) golem.AgentStream[NestedStreamItem] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[NestedStreamItem]) error {
			for i := range uint32(3) {
				item := NestedStreamItem{Label: fmt.Sprintf("item-%d", i), Values: golem.StreamOf(uintRange(i*10, 4)...)}
				if err := w.Write(item); errors.Is(err, golem.ErrReaderGone) {
					return nil
				}
			}
			return nil
		})
	})
	target.Handle(produceSiblings, func(*golem.Context[targetState], golem.Unit) golem.Tuple2[golem.AgentStream[uint32], golem.AgentStream[uint32]] {
		return golem.Tuple2[golem.AgentStream[uint32], golem.AgentStream[uint32]]{
			A: golem.StreamOf(uintRange(0, 192)...),
			B: golem.StreamOf(uintRange(1000, 192)...),
		}
	})
	target.Handle(forwardUnread, func(_ *golem.Context[targetState], in StreamIn) golem.AgentStream[uint32] {
		return in.Input
	})
	target.Handle(produceRecoverableResults, func(*golem.Context[targetState], golem.Unit) golem.AgentStream[golem.Result[uint32, string]] {
		return golem.StreamOf(golem.Ok[uint32, string](1), golem.Err[uint32]("recoverable"), golem.Ok[uint32, string](2))
	})
	target.Handle(produceFailure, func(*golem.Context[targetState], golem.Unit) golem.AgentStream[uint32] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[uint32]) error {
			_ = w.Write(41)
			return errors.New("go stream producer failure")
		})
	})
	target.Handle(produceCancellable, func(*golem.Context[targetState], golem.Unit) golem.AgentStream[uint32] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[uint32]) error {
			if err := w.Write(43); errors.Is(err, golem.ErrReaderGone) {
				return nil
			}
			for {
				if err := w.WriteAll(uintRange(44, 192)...); errors.Is(err, golem.ErrReaderGone) {
					return nil
				}
			}
		})
	})
	target.Handle(produceMany, func(*golem.Context[targetState], golem.Unit) golem.AgentStream[uint32] {
		return golem.StreamOf(uintRange(0, 192)...)
	})
	target.Handle(ping, func(ctx *golem.Context[targetState], _ golem.Unit) string {
		return "pong:" + ctx.State.name
	})
	target.Handle(increment, func(ctx *golem.Context[targetState], _ golem.Unit) uint64 {
		ctx.State.count++
		return ctx.State.count
	})

	caller := Caller.Implement(func(id CallerID) *callerState { return &callerState{targetName: id.TargetName} })
	targetOf := func(ctx *golem.Context[callerState]) golem.Client[TargetID] {
		return Target.Get(TargetID{Name: ctx.State.targetName})
	}
	caller.Handle(run, func(ctx *golem.Context[callerState], _ golem.Unit) GoStreamingRpcReport {
		t := targetOf(ctx)
		var r GoStreamingRpcReport
		r.Consumed = consume.MustCall(t, StreamIn{Input: golem.StreamOf[uint32](1, 2, 3)})
		r.Produced = produce.MustCall(t, golem.Unit{}).Collect()
		r.Transformed = transform.MustCall(t, StreamIn{Input: golem.StreamOf[uint32](4, 5, 6)}).Collect()
		scalar := scalarAndStream.MustCall(t, golem.Unit{})
		r.ScalarLabel, r.ScalarValues = scalar.A, scalar.B.Collect()
		nested := consumeNested.MustCall(t, NestedIn{Input: NestedStreamInput{
			Labels: golem.StreamOf("alpha", "beta", "gamma"),
			Values: golem.StreamOf[uint32](7, 8, 9),
		}})
		r.NestedLabels, r.NestedValues = nested.A, nested.B
		for item := range produceNestedItems.MustCall(t, golem.Unit{}).All() {
			r.NestedItemLabels = append(r.NestedItemLabels, item.Label)
			r.NestedItemValues = append(r.NestedItemValues, item.Values.Collect())
		}
		siblings := produceSiblings.MustCall(t, golem.Unit{})
		r.FirstSibling, r.SecondSibling = both(siblings.A, siblings.B)
		r.Forwarded = forwardUnread.MustCall(t, StreamIn{Input: golem.StreamOf(uintRange(2000, 192)...)}).Collect()
		r.Recoverable = produceRecoverableResults.MustCall(t, golem.Unit{}).Collect()
		early := produceMany.MustCall(t, golem.Unit{})
		r.EarlyValue, _ = early.Next()
		early.Close()
		r.Ping = ping.MustCall(t, golem.Unit{})
		r.Increment = increment.MustCall(t, golem.Unit{})
		return r
	})
	caller.Handle(callProducerFailure, func(ctx *golem.Context[callerState], _ golem.Unit) []uint32 {
		return produceFailure.MustCall(targetOf(ctx), golem.Unit{}).Collect()
	})
	caller.Handle(callProducerCancellation, func(ctx *golem.Context[callerState], _ golem.Unit) golem.Tuple2[uint32, string] {
		t := targetOf(ctx)
		stream := produceCancellable.MustCall(t, golem.Unit{})
		first, _ := stream.Next()
		stream.Close()
		return golem.Tuple2[uint32, string]{A: first, B: ping.MustCall(t, golem.Unit{})}
	})
	caller.Handle(callServerCancellation, func(ctx *golem.Context[callerState], _ golem.Unit) golem.Tuple3[uint32, bool, string] {
		t := targetOf(ctx)
		readerGone := make(chan bool, 1)
		input := golem.ProduceStream(func(w *golem.AgentStreamWriter[uint32]) error {
			for {
				if err := w.WriteAll(uintRange(50, 192)...); errors.Is(err, golem.ErrReaderGone) {
					readerGone <- true
					return nil
				}
			}
		})
		first := consumeOneThenDrop.MustCall(t, StreamIn{Input: input})
		return golem.Tuple3[uint32, bool, string]{A: first, B: <-readerGone, C: ping.MustCall(t, golem.Unit{})}
	})
	caller.Handle(dropOutputEarly, func(ctx *golem.Context[callerState], _ golem.Unit) golem.Tuple2[uint32, string] {
		t := targetOf(ctx)
		stream := produceMany.MustCall(t, golem.Unit{})
		first, _ := stream.Next()
		stream.Close()
		return golem.Tuple2[uint32, string]{A: first, B: ping.MustCall(t, golem.Unit{})}
	})
	caller.Handle(callScalar, func(ctx *golem.Context[callerState], _ golem.Unit) uint64 {
		return increment.MustCall(targetOf(ctx), golem.Unit{})
	})
}
