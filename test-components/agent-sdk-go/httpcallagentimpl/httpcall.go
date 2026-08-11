// Package httpcallagentimpl is the IMPLEMENTATION of the outbound-HTTP agent. The
// SDK routes net/http through the durable wasi:http transport, so the response is
// recorded in the oplog and served from it on replay after a restart rather than
// re-fetched (the exactly-once test asserts this via an external counter).
package httpcallagentimpl

import (
	"io"
	"net/http"
	"os"

	"agent-sdk-go/httpcallagent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var httpAgent = golem.Implement(httpcallagent.Agent, func(httpcallagent.Id) *state { return &state{} })

func init() {
	golem.Handle(httpAgent, httpcallagent.Callback, func(_ *golem.Context[state], in httpcallagent.CallbackIn) string {
		url := "http://localhost:" + os.Getenv("PORT") + "/callback?payload=" + in.Payload
		resp := golem.Must(http.Get(url))
		defer resp.Body.Close()
		return string(golem.Must(io.ReadAll(resp.Body)))
	})
}
