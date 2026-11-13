// Package fixture is the Go fixture of the streaming fault matrix. It implements the
// fixture contract documented in integration-tests/src/fault_matrix.rs. Record seq of
// run run is the number run*100_000 + seq, for seq in 1..=count.
package fixture

import (
	"errors"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type ID struct{ Name string }

// GateComponentID, GateAgentID and Gate have the schema shape of the WIT promise-id record,
// which nests the component id as a record. golem.PromiseID flattens the component id to a
// UUID, so another SDK's promise id does not decode into it.
type GateComponentID struct{ UUID golem.UUID }

type GateAgentID struct {
	ComponentID GateComponentID
	AgentID     string
}

type Gate struct {
	AgentID  GateAgentID
	OplogIdx uint64
}

func gateOf(id golem.PromiseID) Gate {
	return Gate{
		AgentID:  GateAgentID{ComponentID: GateComponentID{UUID: id.AgentID.ComponentID}, AgentID: id.AgentID.AgentID},
		OplogIdx: id.OplogIndex,
	}
}

func (g Gate) promiseID() golem.PromiseID {
	return golem.PromiseID{
		AgentID:    golem.AgentID{ComponentID: g.AgentID.ComponentID.UUID, AgentID: g.AgentID.AgentID},
		OplogIndex: g.OplogIdx,
	}
}

type ProduceIn struct {
	Run        uint32
	Count      uint32
	PauseAfter uint32
	Gate       Gate
}

type FailIn struct {
	Run       uint32
	FailAfter uint32
}

type ConsumeIn struct{ Input golem.AgentStream[uint32] }

type DropIn struct {
	Input golem.AgentStream[uint32]
	Keep  uint32
}

type PipeIn struct {
	Target string
	Run    uint32
	Count  uint32
	Keep   uint32
}

type ReadRemoteIn struct {
	Target     string
	Run        uint32
	Count      uint32
	PauseAfter uint32
	Gate       Gate
}

var Fixture = golem.DefineAgent[ID](golem.Spec{Name: "FaultMatrixFixture"})

var (
	createGate      = Fixture.Method[golem.Unit, Gate]("create_gate")
	produce         = Fixture.Method[ProduceIn, golem.AgentStream[uint32]]("produce")
	produceThenFail = Fixture.Method[FailIn, golem.AgentStream[uint32]]("produce_then_fail")
	consume         = Fixture.Method[ConsumeIn, []uint32]("consume")
	consumeThenDrop = Fixture.Method[DropIn, []uint32]("consume_then_drop")
	pipe            = Fixture.Method[PipeIn, []uint32]("pipe")
	readRemote      = Fixture.Method[ReadRemoteIn, []uint32]("read_remote")
)

func record(run, seq uint32) uint32 { return run*100_000 + seq }

func take(input golem.AgentStream[uint32], keep uint32) []uint32 {
	defer input.Close()
	records := []uint32{}
	for uint32(len(records)) < keep {
		next, ok := input.Next()
		if !ok {
			break
		}
		records = append(records, next)
	}
	return records
}

type state struct{}

func init() {
	fixture := Fixture.Implement(func(ID) *state { return &state{} })
	fixture.Handle(createGate, func(*golem.Context[state], golem.Unit) Gate {
		return gateOf(golem.NewPromise[[]byte]().ID())
	})
	fixture.Handle(produce, func(_ *golem.Context[state], in ProduceIn) golem.AgentStream[uint32] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[uint32]) error {
			for seq := uint32(1); seq <= in.Count; seq++ {
				if seq == in.PauseAfter+1 {
					golem.PromiseByID[[]byte](in.Gate.promiseID()).MustAwait()
				}
				if err := w.Write(record(in.Run, seq)); errors.Is(err, golem.ErrReaderGone) {
					return nil
				}
			}
			return nil
		})
	})
	fixture.Handle(produceThenFail, func(_ *golem.Context[state], in FailIn) golem.AgentStream[uint32] {
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[uint32]) error {
			for seq := uint32(1); seq <= in.FailAfter; seq++ {
				if err := w.Write(record(in.Run, seq)); errors.Is(err, golem.ErrReaderGone) {
					return nil
				}
			}
			return errors.New("the fixture producer failed")
		})
	})
	fixture.Handle(consume, func(_ *golem.Context[state], in ConsumeIn) []uint32 {
		return in.Input.Collect()
	})
	fixture.Handle(consumeThenDrop, func(_ *golem.Context[state], in DropIn) []uint32 {
		return take(in.Input, in.Keep)
	})
	fixture.Handle(pipe, func(_ *golem.Context[state], in PipeIn) []uint32 {
		records := make([]uint32, in.Count)
		for i := range records {
			records[i] = record(in.Run, uint32(i)+1)
		}
		target := Fixture.Get(ID{Name: in.Target})
		if in.Keep >= in.Count {
			return consume.MustCall(target, ConsumeIn{Input: golem.StreamOf(records...)})
		}
		return consumeThenDrop.MustCall(target, DropIn{Input: golem.StreamOf(records...), Keep: in.Keep})
	})
	fixture.Handle(readRemote, func(_ *golem.Context[state], in ReadRemoteIn) []uint32 {
		output := produce.MustCall(Fixture.Get(ID{Name: in.Target}), ProduceIn{
			Run: in.Run, Count: in.Count, PauseAfter: in.PauseAfter, Gate: in.Gate,
		})
		return output.Collect()
	})
}
