// Package configechoagent is the DEFINITION of the configured echo agent. The
// behaviour lives in configechoagentimpl.
package configechoagent

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type FeeConfig struct{ Cents int64 }

// Config flattens to per-key paths (field names lower-cased): Greeting -> "greeting",
// Fee.Cents -> "fee"/"cents".
type Config struct {
	Greeting string
	Fee      FeeConfig
}

var Agent = golem.DefineConfiguredAgent[Id, Config](golem.Spec{
	Name: "ConfigAgent", Description: "Echoes agent config at runtime", Mode: golem.Durable,
})

var (
	Greeting = golem.DefineMethod[Id, golem.Unit, string]("greeting",
		golem.Desc("Return the configured greeting"))
	Cents = golem.DefineMethod[Id, golem.Unit, int64]("cents",
		golem.Desc("Return the configured fee (nested config path)"))
)
