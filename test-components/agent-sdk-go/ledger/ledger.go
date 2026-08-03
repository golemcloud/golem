// Package ledger is the RPC callee: a durable per-region ledger the caller agent
// invokes via cross-agent RPC. Its accumulating state proves the RPC reached a
// real durable target and mutated it.
package ledger

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Region string }
type State struct{ total int64 }

var Agent = golem.DefineAgent[Id, State](
	golem.Spec{Name: "LedgerAgent", Description: "Per-region revenue ledger", Mode: golem.Durable},
	func(Id) *State { return &State{} },
)

type RecordIn struct{ Amount int64 }

var (
	Record = golem.DefineMethod[Id, RecordIn, golem.Result[int64, string]]("record",
		golem.Desc("Add to the ledger, returning the new total or an error value"))
	Total = golem.DefineMethod[Id, golem.Unit, int64]("total")
)

func init() {
	golem.Implement(Agent, Record, func(ctx *golem.Context[State], in RecordIn) golem.Result[int64, string] {
		if in.Amount < 0 {
			return golem.Err[int64, string]("cannot record a negative amount")
		}
		ctx.State.total += in.Amount
		return golem.Ok[int64, string](ctx.State.total)
	})
	golem.Implement(Agent, Total, func(ctx *golem.Context[State], _ golem.Unit) int64 { return ctx.State.total })
}
