// Package impl is the IMPLEMENTATION of the outbound-HTTP agent. The
// SDK routes net/http through the durable wasi:http transport, so the response is
// recorded in the oplog and served from it on replay after a restart rather than
// re-fetched (the exactly-once test asserts this via an external counter).
package impl

import (
	"io"
	"net/http"
	"os"

	"agent-sdk-go/agents/httpcall"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

func fetch(payload string) string {
	url := "http://localhost:" + os.Getenv("PORT") + "/callback?payload=" + payload
	resp := golem.Must(http.Get(url))
	defer resp.Body.Close()
	return string(golem.Must(io.ReadAll(resp.Body)))
}

var agent = golem.Implement(httpcall.Agent, func(httpcall.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, httpcall.Callback, func(_ *golem.Context[state], in httpcall.CallbackIn) string {
		return fetch(in.Payload)
	})
	golem.Handle(agent, httpcall.AtomicCallback, func(_ *golem.Context[state], in httpcall.CallbackIn) string {
		var body string
		golem.Atomically(func() { body = fetch(in.Payload) })
		return body
	})
}
