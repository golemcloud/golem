// Package httpcall is a durable outbound-HTTP agent for the replay tests: it GETs
// a host-provided callback endpoint and returns its body. Because the SDK routes
// net/http through the durable wasi:http transport, the response is recorded in
// the oplog — so on replay after a restart it is served from the oplog rather
// than re-fetched, which the exactly-once test asserts via an external counter.
package httpcall

import (
	"io"
	"net/http"
	"os"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type Id struct{ Name string }
type State struct{}

var Agent = golem.DefineAgent[Id, State](
	golem.Spec{Name: "HttpAgent", Description: "Durable outbound HTTP for replay tests", Mode: golem.Durable},
	func(Id) *State { return &State{} },
)

type CallbackIn struct{ Payload string }

var Callback = golem.DefineMethod[Id, CallbackIn, string]("callback",
	golem.Desc("GET the PORT callback endpoint with the payload and return its body"))

func init() {
	golem.Implement(Agent, Callback, func(_ *golem.Context[State], in CallbackIn) string {
		url := "http://localhost:" + os.Getenv("PORT") + "/callback?payload=" + in.Payload
		resp := golem.Must(http.Get(url))
		defer resp.Body.Close()
		return string(golem.Must(io.ReadAll(resp.Body)))
	})
}
