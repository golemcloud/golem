// Package httpcall is the DEFINITION of the durable outbound-HTTP agent used
// by the replay tests. The behaviour lives in httpcall/impl.
package httpcall

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type CallbackIn struct{ Payload string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "HttpAgent", Description: "Durable outbound HTTP for replay tests", Mode: golem.Durable,
})

var Callback = golem.DefineMethod[Id, CallbackIn, string]("callback",
	golem.Desc("GET the PORT callback endpoint with the payload and return its body"))
