// Package impl is the IMPLEMENTATION of the ledger agent.
package impl

import (
	"agent-sdk-go/agents/ledger"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ total int64 }

var agent = golem.Implement(ledger.Agent, func(ledger.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, ledger.Record, func(ctx *golem.Context[state], in ledger.RecordIn) golem.Result[int64, string] {
		if in.Amount < 0 {
			return golem.Err[int64, string]("cannot record a negative amount")
		}
		ctx.State.total += in.Amount
		return golem.Ok[int64, string](ctx.State.total)
	})
	golem.Handle(agent, ledger.Total, func(ctx *golem.Context[state], _ golem.Unit) int64 { return ctx.State.total })
}
