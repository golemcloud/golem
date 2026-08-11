// Package configechoagentimpl is the IMPLEMENTATION of the configured echo agent:
// its methods read the agent config with golem.Config. Importing it registers the
// agent.
package configechoagentimpl

import (
	"agent-sdk-go/configechoagent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var _ = golem.Implement(configechoagent.Agent,
	func(configechoagent.Id) *state { return &state{} },
	golem.Bound(configechoagent.Greeting, func(ctx *golem.Context[state], _ golem.Unit) string {
		return golem.Config(configechoagent.Agent, ctx).Greeting
	}),
	golem.Bound(configechoagent.Cents, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return golem.Config(configechoagent.Agent, ctx).Fee.Cents
	}),
)
