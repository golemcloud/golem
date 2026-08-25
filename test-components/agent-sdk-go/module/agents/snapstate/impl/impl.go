// Package impl is the IMPLEMENTATION of the custom-snapshot agent. The counter
// is deliberately an UNEXPORTED field, which the SDK's default reflective JSON
// snapshot cannot see — so the state survives snapshot-based recovery only if the
// SDK actually calls this type's Save/Load (the golem.Snapshotter path).
package impl

import (
	"encoding/json"

	"agent-sdk-go/agents/snapstate"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ count int64 }

// Save/Load implement golem.Snapshotter over the unexported counter.
func (s *state) Save() ([]byte, error) { return json.Marshal(s.count) }
func (s *state) Load(b []byte) error   { return json.Unmarshal(b, &s.count) }

var agent = golem.Implement(snapstate.Agent, func(snapstate.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, snapstate.Bump, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.count++
		return ctx.State.count
	})
	golem.Handle(agent, snapstate.Value, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return ctx.State.count
	})
}
