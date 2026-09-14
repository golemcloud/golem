// Package impl is the IMPLEMENTATION of the session agent: its private state,
// method handlers, and custom snapshot serialization. Importing it registers the
// agent.
package impl

import (
	"component-name/agents/session"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// state keeps a running total in an unexported field. Reflection cannot see
// unexported fields, so the default snapshot (JSON of exported fields) would drop
// it; implementing golem.Snapshotter below is what makes the state survive.
type state struct{ total int64 }

// Save/Load make state a golem.Snapshotter: the returned bytes are stored
// verbatim by the host and handed back on restore, so unexported state is kept.
func (s *state) Save() ([]byte, error) { return []byte{byte(s.total)}, nil }
func (s *state) Load(b []byte) error {
	if len(b) > 0 {
		s.total = int64(b[0])
	}
	return nil
}

var agent = golem.Implement(session.Agent, func(session.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, session.Spend, func(ctx *golem.Context[state], in session.SpendIn) int64 {
		ctx.State.total += in.Amount
		return ctx.State.total
	})
	golem.Handle(agent, session.Total, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return ctx.State.total
	})
}
