// Package wsecho is the DEFINITION of the agent exercising the SDK's websocket
// wrapper (golem/websocket). Behaviour lives in wsecho/impl.
package wsecho

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type EchoIn struct {
	URL     string
	Message string
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "WsAgent", Description: "Exercises the Go SDK websocket wrapper", Mode: golem.Durable,
})

var Echo = golem.DefineMethod[Id, EchoIn, string]("echo",
	golem.Desc("Connect to the URL, send the message, and return the echoed reply"))
