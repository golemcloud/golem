// Package logs is the DEFINITION of the agent exercising the SDK's logging:
// slog and the standard log package reach the host logging channel with their
// levels. Behaviour lives in logs/impl.
package logger

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type EmitIn struct{ Tag string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "LogAgent", Description: "Exercises the Go SDK logging", Mode: golem.Durable,
})

var Emit = Agent.Method[EmitIn, golem.Unit]("emit", golem.Description("Log one record per level"))
