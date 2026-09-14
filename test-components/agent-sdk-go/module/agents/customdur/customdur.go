// Package customdur is the DEFINITION of an agent that exercises the custom
// durability API (golem.DurableOp): it wraps an outbound HTTP side effect so the
// operation is recorded once and replayed from the oplog after a restart rather
// than re-run. Behaviour lives in customdur/impl.
package customdur

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type CallbackIn struct{ Payload string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "CustomDurAgent", Description: "Custom durable operation (DurableOp) for replay tests", Mode: golem.Durable,
})

var Callback = golem.DefineMethod[Id, CallbackIn, string]("callback",
	golem.Desc("Wrap an outbound HTTP call in golem.DurableOp and return its body"))
