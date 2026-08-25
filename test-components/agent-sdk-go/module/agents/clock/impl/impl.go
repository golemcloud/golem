// Package impl is the IMPLEMENTATION of the durable-clock agent. time.Now() is
// routed through the durable host clock, so a reading recorded in one invocation
// is reproduced from the oplog when that invocation is replayed after a restart
// (rather than re-reading the current time). The replay test asserts this by
// checking the first recorded reading is unchanged across an executor restart.
package impl

import (
	"time"

	"agent-sdk-go/agents/clock"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ times []int64 }

var agent = golem.Implement(clock.Agent, func(clock.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, clock.RecordTime, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		now := time.Now().UnixNano()
		ctx.State.times = append(ctx.State.times, now)
		return now
	})
	golem.Handle(agent, clock.FirstTime, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return ctx.State.times[0]
	})
}
