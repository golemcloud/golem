// Package impl is the IMPLEMENTATION of the counter agent: its
// private state and method handlers. Importing it registers the agent.
package impl

import (
	"agent-sdk-go/agents/counter"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ value int64 }

func (s *state) current() int64 { return s.value }

var agent = golem.Implement(counter.Agent, func(counter.CounterID) *state { return &state{} })

func init() {
	golem.Handle(agent, counter.Increment, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.value++
		return ctx.State.value
	})
	golem.Handle(agent, counter.Add, func(ctx *golem.Context[state], in counter.AddIn) int64 {
		ctx.State.value += in.By
		return ctx.State.value
	})
	golem.Handle(agent, counter.Value, golem.Bind0((*state).current)) // method-expression binding
}
