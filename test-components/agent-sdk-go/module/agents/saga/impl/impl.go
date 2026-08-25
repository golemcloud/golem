// Package impl is the IMPLEMENTATION of the saga agent. Each step and each
// compensation appends its name to the agent's state, so a caller can read back
// exactly what ran and in what order: a committed run records only the forward
// steps, while a rolled-back run also records the compensations of the steps that
// had already succeeded, in reverse order.
package impl

import (
	"agent-sdk-go/agents/saga"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ log []string }

var agent = golem.Implement(saga.Agent, func(saga.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, saga.Run, func(ctx *golem.Context[state], in saga.RunIn) string {
		s := ctx.State
		record := func(name string) { s.log = append(s.log, name) }

		// charge succeeds and knows how to undo itself (refund).
		charge := golem.NewOperation(
			func(_ golem.Unit) golem.Result[string, string] {
				record("charge")
				return golem.Ok[string, string]("charged")
			},
			func(_ golem.Unit, _ string) golem.Result[golem.Unit, string] {
				record("refund")
				return golem.Ok[golem.Unit, string](golem.Unit{})
			},
		)
		// ship records its attempt, then fails when the caller asked it to.
		ship := golem.NewOperation(
			func(fail bool) golem.Result[string, string] {
				record("ship")
				if fail {
					return golem.Err[string, string]("ship failed")
				}
				return golem.Ok[string, string]("shipped")
			},
			func(bool, string) golem.Result[golem.Unit, string] {
				record("unship")
				return golem.Ok[golem.Unit, string](golem.Unit{})
			},
		)

		res := golem.FallibleTransaction(func(tx *golem.Transaction[string]) golem.Result[string, string] {
			if r := golem.Step(tx, charge, golem.Unit{}); r.IsErr() {
				return golem.Err[string, string](r.Err())
			}
			if r := golem.Step(tx, ship, in.Fail); r.IsErr() {
				return golem.Err[string, string](r.Err())
			}
			return golem.Ok[string, string]("committed")
		})

		if res.IsErr() {
			return "rolled-back"
		}
		return res.Ok()
	})
	golem.Handle(agent, saga.Log, func(ctx *golem.Context[state], _ golem.Unit) []string {
		return ctx.State.log
	})
}
