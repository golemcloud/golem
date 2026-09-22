// Package ledger is the DEFINITION of the per-region ledger agent (the RPC
// callee). The behaviour and state live in ledger/impl.
package ledger

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Region string }

type RecordIn struct{ Amount int64 }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "LedgerAgent", Description: "Per-region revenue ledger", Mode: golem.Durable,
})

var (
	Record = Agent.Method[RecordIn, golem.Result[int64, string]]("record", golem.Desc("Add to the ledger, returning the new total or an error value"))
	Total  = Agent.Method[golem.Unit, int64]("total")
)
