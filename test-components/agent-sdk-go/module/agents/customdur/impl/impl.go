// Package impl is the IMPLEMENTATION of the custom-durability agent. The handler
// wraps a raw outbound HTTP call in durability.Run, so the call's result is
// recorded once and, after an executor restart, replayed from the oplog without
// re-running the body — the replay test asserts the external counter advances
// once per live call, not on replay.
package impl

import (
	"io"
	"net/http"
	"os"

	"agent-sdk-go/agents/customdur"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durability"
)

type state struct{}

var agent = customdur.Agent.Implement(func(customdur.Id) *state { return &state{} })

func init() {
	agent.Handle(customdur.Callback, func(_ *golem.Context[state], in customdur.CallbackIn) string {
		return durability.Run(
			durability.Spec{Interface: "agent-sdk-go", Function: "custom-callback", Type: durability.WriteRemote},
			in,
			func() string {
				url := "http://localhost:" + os.Getenv("PORT") + "/callback?payload=" + in.Payload
				resp := golem.Must(http.Get(url))
				defer resp.Body.Close()
				return string(golem.Must(io.ReadAll(resp.Body)))
			},
		)
	})
}
