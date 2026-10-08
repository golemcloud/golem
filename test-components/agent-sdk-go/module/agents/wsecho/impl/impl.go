// Package impl is the IMPLEMENTATION of the websocket agent: connect, send one
// message, read the echoed reply, and close — the whole client lifecycle of the
// SDK's websocket wrapper in one invocation.
package impl

import (
	"agent-sdk-go/agents/wsecho"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/websocket"
)

type state struct{}

var agent = wsecho.Agent.Implement(func(wsecho.Id) *state { return &state{} })

func init() {
	agent.Handle(wsecho.Echo, func(_ *golem.Context[state], in wsecho.EchoIn) string {
		conn := websocket.MustConnect(in.URL)
		conn.MustSendText(in.Message)
		msg := golem.Must(conn.Receive())
		golem.Must0(conn.Close(1000, "done"))
		return msg.Text()
	})
}
