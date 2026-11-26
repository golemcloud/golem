// Package fixture is the Go fixture of the streaming fault matrix. It implements the
// fixture contract documented in integration-tests/src/fault_matrix.rs. Record seq of
// run run is the number run*100_000 + seq, for seq in 1..=count.
package fixture

import (
	"encoding/binary"
	"errors"
	"io"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

type ID struct{ Name string }

// GateAgentID and Gate have the schema shape of the WIT promise-id record, which nests the
// agent id as a { uuid } record. golem.AgentID is an opaque UUID whose schema is not that
// record, so another SDK's promise id does not decode into golem.PromiseID.
type GateAgentID struct{ UUID golem.UUID }

type Gate struct {
	AgentID     GateAgentID
	Fingerprint golem.UUID
	OplogIdx    uint64
}

func gateOf(id golem.PromiseID) Gate {
	return Gate{
		AgentID:     GateAgentID{UUID: id.AgentID.UUID()},
		Fingerprint: id.Fingerprint,
		OplogIdx:    id.OplogIndex,
	}
}

func (g Gate) promiseID() golem.PromiseID {
	return golem.PromiseID{
		AgentID:     golem.AgentIDFromUUID(g.AgentID.UUID),
		Fingerprint: g.Fingerprint,
		OplogIndex:  g.OplogIdx,
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
		next, err := input.Next()
		if errors.Is(err, io.EOF) {
			break
		}
		records = append(records, golem.Must(next, err))
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
		return golem.Must(in.Input.Collect())
	})
	fixture.Handle(consumeThenDrop, func(_ *golem.Context[state], in DropIn) []uint32 {
		return take(in.Input, in.Keep)
	})
	fixture.Handle(pipe, func(_ *golem.Context[state], in PipeIn) []uint32 {
		records := make([]uint32, in.Count)
		for i := range records {
			records[i] = record(in.Run, uint32(i)+1)
		}
		target := Fixture.Upsert(ID{Name: in.Target})
		if in.Keep >= in.Count {
			return consume.MustCall(target, ConsumeIn{Input: golem.StreamOf(records...)})
		}
		return consumeThenDrop.MustCall(target, DropIn{Input: golem.StreamOf(records...), Keep: in.Keep})
	})
	fixture.Handle(readRemote, func(_ *golem.Context[state], in ReadRemoteIn) []uint32 {
		output := produce.MustCall(Fixture.Upsert(ID{Name: in.Target}), ProduceIn{
			Run: in.Run, Count: in.Count, PauseAfter: in.PauseAfter, Gate: in.Gate,
		})
		return golem.Must(output.Collect())
	})
}

// FaultMatrix is the tool of the fixture contract: its produce command writes the records of
// produce to stdout, each as 4 big-endian bytes.
type FaultMatrix struct{}

var Tool = tool.DefineTool[FaultMatrix]("fault-matrix", tool.Spec{Version: "1.0.0"})

type ToolProduceArgs struct {
	Run        uint32
	Count      uint32
	PauseAfter uint32
	Gate       Gate
}

var toolProduce = Tool.OutputCommand[ToolProduceArgs, uint32]("produce", func(a *ToolProduceArgs, s *tool.CommandSpec) {
	s.Positional(&a.Run)
	s.Positional(&a.Count)
	s.Positional(&a.PauseAfter)
	s.Positional(&a.Gate)
	s.Stdout().Required()
})

var _ = toolProduce.Handle(func(ctx *tool.OutputContext, a ToolProduceArgs) (uint32, error) {
	for seq := uint32(1); seq <= a.Count; seq++ {
		if seq == a.PauseAfter+1 {
			golem.PromiseByID[[]byte](a.Gate.promiseID()).MustAwait()
		}
		if _, err := ctx.Stdout().Write(binary.BigEndian.AppendUint32(nil, record(a.Run, seq))); err != nil {
			return seq - 1, nil
		}
	}
	return a.Count, nil
})
