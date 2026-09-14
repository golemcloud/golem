// Package impl is the IMPLEMENTATION of the configured echo agent:
// its methods read the agent config with golem.Config. Its constructor doesn't
// need config, so it uses plain Implement. Importing it registers the agent.
package impl

import (
	"agent-sdk-go/agents/configecho"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(configecho.Agent, func(configecho.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, configecho.Greeting, func(ctx *golem.Context[state], _ golem.Unit) string {
		return golem.Config(configecho.Agent, ctx).Greeting
	})
	golem.Handle(agent, configecho.Cents, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return golem.Config(configecho.Agent, ctx).Fee.Cents
	})
}
