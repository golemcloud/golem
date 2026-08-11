// Package configechoagentimpl is the IMPLEMENTATION of the configured echo agent:
// its methods read the agent config with golem.Config. Its constructor doesn't
// need config, so it uses plain Implement. Importing it registers the agent.
package configechoagentimpl

import (
	"agent-sdk-go/configechoagent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var configEcho = golem.Implement(configechoagent.Agent, func(configechoagent.Id) *state { return &state{} })

func init() {
	golem.Handle(configEcho, configechoagent.Greeting, func(ctx *golem.Context[state], _ golem.Unit) string {
		return golem.Config(configechoagent.Agent, ctx).Greeting
	})
	golem.Handle(configEcho, configechoagent.Cents, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		return golem.Config(configechoagent.Agent, ctx).Fee.Cents
	})
}
