// Package counteragentimpl is the IMPLEMENTATION of the counter agent: its
// private state and method handlers. Importing it registers the agent.
package counteragentimpl

import (
	"agent-sdk-go/counteragent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ value int64 }

func (s *state) current() int64 { return s.value }

var counter = golem.Implement(counteragent.Agent, func(counteragent.CounterID) *state { return &state{} })

func init() {
	golem.Handle(counter, counteragent.Increment, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.value++
		return ctx.State.value
	})
	golem.Handle(counter, counteragent.Add, func(ctx *golem.Context[state], in counteragent.AddIn) int64 {
		ctx.State.value += in.By
		return ctx.State.value
	})
	golem.Handle(counter, counteragent.Value, golem.Bind0((*state).current)) // method-expression binding
}
