// Package impl is the IMPLEMENTATION of the custom-snapshot agent. The counter
// is deliberately an UNEXPORTED field, which the SDK's default reflective JSON
// snapshot cannot see — so the state survives snapshot-based recovery only if the
// SDK actually calls this type's Save/Load (the golem.Snapshotter path). origin
// is runtime-only: the constructor and the restore hook each append to it, so it
// shows whether a restore ran the constructor.
package impl

import (
	"encoding/json"

	"agent-sdk-go/agents/snapstate"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct {
	count  int64
	origin string
}

// Save/Load implement golem.Snapshotter over the unexported counter.
func (s *state) Save() ([]byte, error) { return json.Marshal(s.count) }
func (s *state) Load(b []byte) error   { return json.Unmarshal(b, &s.count) }

var agent = snapstate.Agent.Implement(func(snapstate.Id) *state { return &state{origin: "constructed;"} })

func init() {
	agent.OnRestore(func(_ *golem.InitContext[snapstate.Id, state, golem.NoConfig], s *state) error {
		s.origin += "restored"
		return nil
	})
	agent.Handle(snapstate.Bump, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.count++
		return ctx.State.count
	})
	agent.Handle(snapstate.Value, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return ctx.State.count
	})
	agent.Handle(snapstate.Origin, func(ctx *golem.Context[state], _ golem.Unit) string {
		return ctx.State.origin
	})
}
