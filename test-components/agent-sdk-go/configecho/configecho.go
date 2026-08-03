// Package configecho is a configured agent that echoes its agent-config values,
// proving the runtime config-read path: values provided at deploy (flat and
// nested) are resolved and returned by the agent under the executor.
package configecho

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }
type State struct{}

type FeeConfig struct{ Cents int64 }

// Config flattens to per-key paths (field names lower-cased): Greeting -> "greeting",
// Fee.Cents -> "fee"/"cents".
type Config struct {
	Greeting string
	Fee      FeeConfig
}

var Agent = golem.DefineConfiguredAgent[Id, State, Config](
	golem.Spec{Name: "ConfigAgent", Description: "Echoes agent config at runtime", Mode: golem.Durable},
	func(*golem.InitContext[Id, State, Config]) *State { return &State{} },
)

var (
	Greeting = golem.DefineMethod[Id, golem.Unit, string]("greeting",
		golem.Desc("Return the configured greeting"))
	Cents = golem.DefineMethod[Id, golem.Unit, int64]("cents",
		golem.Desc("Return the configured fee (nested config path)"))
)

func init() {
	golem.Implement(Agent, Greeting, func(ctx *golem.Context[State], _ golem.Unit) string {
		return Agent.Config(ctx).Greeting
	})
	golem.Implement(Agent, Cents, func(ctx *golem.Context[State], _ golem.Unit) int64 {
		return Agent.Config(ctx).Fee.Cents
	})
}
