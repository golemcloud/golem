// Package impl is the IMPLEMENTATION of the scheduler agent. It schedules a
// cross-agent invocation for a moment in the future; the executor runs it without
// the caller waiting, so the tests observe the effect by polling the target
// counter. Cancelling the returned handle must prevent the invocation entirely.
package impl

import (
	"time"

	"agent-sdk-go/agents/counter"
	"agent-sdk-go/agents/scheduler"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(scheduler.Agent, func(scheduler.Id) *state { return &state{} })

func scheduleBump(in scheduler.ScheduleIn) *golem.ScheduledInvocation {
	c := counter.Agent.Get(counter.CounterID{Name: in.Target})
	at := time.Now().Add(time.Duration(in.DelayMillis) * time.Millisecond)
	return counter.Increment.Schedule(c, at, golem.Unit{})
}

func init() {
	golem.Handle(agent, scheduler.Bump, func(_ *golem.Context[state], in scheduler.ScheduleIn) golem.Unit {
		scheduleBump(in)
		return golem.Unit{}
	})
	golem.Handle(agent, scheduler.BumpCancelled, func(_ *golem.Context[state], in scheduler.ScheduleIn) golem.Unit {
		scheduleBump(in).Cancel()
		return golem.Unit{}
	})
}
