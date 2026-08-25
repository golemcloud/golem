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

var agent = golem.Implement(wsecho.Agent, func(wsecho.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, wsecho.Echo, func(_ *golem.Context[state], in wsecho.EchoIn) string {
		conn := golem.Must(websocket.Connect(in.URL))
		golem.Must0(conn.SendText(in.Message))
		msg := golem.Must(conn.Receive())
		golem.Must0(conn.Close(1000, "done"))
		return msg.Text()
	})
}
