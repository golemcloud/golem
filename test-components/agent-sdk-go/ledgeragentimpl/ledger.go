// Package ledgeragentimpl is the IMPLEMENTATION of the ledger agent. Importing it
// registers the agent.
package ledgeragentimpl

import (
	"agent-sdk-go/ledgeragent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ total int64 }

var _ = golem.Implement(ledgeragent.Agent,
	func(ledgeragent.Id) *state { return &state{} },
	golem.Bound(ledgeragent.Record, func(ctx *golem.Context[state], in ledgeragent.RecordIn) golem.Result[int64, string] {
		if in.Amount < 0 {
			return golem.Err[int64, string]("cannot record a negative amount")
		}
		ctx.State.total += in.Amount
		return golem.Ok[int64, string](ctx.State.total)
	}),
	golem.Bound(ledgeragent.Total, func(ctx *golem.Context[state], _ golem.Unit) int64 { return ctx.State.total }),
)
