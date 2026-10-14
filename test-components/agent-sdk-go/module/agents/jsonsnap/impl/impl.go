// Package impl is the IMPLEMENTATION of the default-snapshot agent. Count and
// Items are exported, so the default JSON snapshot carries them; origin is
// unexported and runtime-only, so it shows whether a restore ran the
// constructor or the restore hook.
package impl

import (
	"agent-sdk-go/agents/jsonsnap"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct {
	Count  int64
	Items  []string
	origin string
}

var agent = jsonsnap.Agent.Implement(func(jsonsnap.Id) *state { return &state{origin: "constructed"} })

func init() {
	agent.OnRestore(func(_ *golem.InitContext[jsonsnap.Id, state, golem.NoConfig], s *state) error {
		s.origin = "restored"
		return nil
	})
	agent.Handle(jsonsnap.Add, func(ctx *golem.Context[state], in jsonsnap.AddIn) int64 {
		ctx.State.Count++
		ctx.State.Items = append(ctx.State.Items, in.Item)
		return ctx.State.Count
	})
	agent.Handle(jsonsnap.Items, func(ctx *golem.Context[state], _ golem.Unit) []string {
		return ctx.State.Items
	})
	agent.Handle(jsonsnap.Origin, func(ctx *golem.Context[state], _ golem.Unit) string {
		return ctx.State.origin
	})
}
