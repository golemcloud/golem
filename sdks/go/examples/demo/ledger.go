package main

import "github.com/golemcloud/golem/sdks/go/golem"

// LedgerAgent records revenue per region. It is the target for the shop's
// fire-and-forget (Trigger), fan-out (CallAsync) and scheduled (Schedule) calls.
type LedgerId struct{ Region string }
type LedgerState struct{ total int64 }

var Ledger = golem.DefineAgent[LedgerId, LedgerState](
	golem.Spec{Name: "LedgerAgent", Description: "Per-region revenue ledger", Mode: golem.Durable},
	func(id LedgerId) *LedgerState { return &LedgerState{} },
)

type RecordIn struct{ Amount int64 }

var (
	// Record returns a Result: an expected typed outcome (a value), not a failure.
	Record = golem.DefineMethod[LedgerId, RecordIn, golem.Result[int64, string]](
		"record",
		golem.Desc("Add to the ledger, returning the new total or an error value"),
	)
	Total = golem.DefineMethod[LedgerId, golem.Unit, int64]("total")
)

func init() {
	golem.Implement(Ledger, Record, func(ctx *golem.Context[LedgerState], in RecordIn) golem.Result[int64, string] {
		if in.Amount < 0 {
			return golem.Err[int64, string]("cannot record a negative amount")
		}
		ctx.State.total += in.Amount
		return golem.Ok[int64, string](ctx.State.total)
	})

	// A plain Go method bound with a method expression.
	golem.Implement(Ledger, Total, golem.Bind0((*LedgerState).snapshot))
}

func (s *LedgerState) snapshot() int64 { return s.total }
