// Package ledgeragentimpl is the IMPLEMENTATION of the ledger agent.
package ledgeragentimpl

import (
	"agent-sdk-go/ledgeragent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ total int64 }

var ledger = golem.Implement(ledgeragent.Agent, func(ledgeragent.Id) *state { return &state{} })

func init() {
	golem.Handle(ledger, ledgeragent.Record, func(ctx *golem.Context[state], in ledgeragent.RecordIn) golem.Result[int64, string] {
		if in.Amount < 0 {
			return golem.Err[int64, string]("cannot record a negative amount")
		}
		ctx.State.total += in.Amount
		return golem.Ok[int64, string](ctx.State.total)
	})
	golem.Handle(ledger, ledgeragent.Total, func(ctx *golem.Context[state], _ golem.Unit) int64 { return ctx.State.total })
}
