// Package native is the Go consumer driven by go_native_tool.rs: it calls the
// server's ambient native conformance tool through the generated guest tool
// client.
package native

import (
	"errors"
	"fmt"

	conformance "golem.local/bridge/native-conformance-tool-guest-client"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

type ID struct{ Name string }

var Consumer = golem.DefineAgent[ID](golem.Spec{Name: "NativeGoConsumer"})

var Exercise = Consumer.Method[golem.Unit, []string]("exercise")

type UnauthorizedID struct{ Name string }

var Unauthorized = golem.DefineAgent[UnauthorizedID](golem.Spec{Name: "UnauthorizedNativeGoConsumer"})

var Denied = Unauthorized.Method[golem.Unit, string]("exercise")

type state struct{}

func check(ok bool, format string, args ...any) {
	if !ok {
		panic(fmt.Sprintf(format, args...))
	}
}

func init() {
	consumer := Consumer.Implement(func(ID) *state { return &state{} })
	consumer.Handle(Exercise, func(_ *golem.Context[state], _ golem.Unit) []string {
		structured, err := conformance.Structured.Call(func(a *conformance.StructuredArgs) {
			a.Value = "alpha"
			a.Count = 7
		})
		check(err == nil, "structured: %v", err)

		supported := "error:missing"
		if _, err := conformance.SupportedError.Call(func(a *conformance.SupportedErrorArgs) {
			a.Reason = "expected"
		}); err != nil {
			supported = "error:unexpected"
			if rejected, ok := conformance.ErrRejected.Match(err); ok {
				supported = "error:rejected:" + rejected
			}
		}

		inv, err := conformance.FiniteStream.Call(func(a *conformance.FiniteStreamArgs) { a.Value = "payload" })
		check(err == nil, "finite stream: %v", err)
		stream := inv.Collect()
		check(stream.Err == nil && stream.StdoutErr == nil, "finite stream: %v, %v", stream.Err, stream.StdoutErr)

		middleware, err := conformance.Middleware.Call(func(a *conformance.MiddlewareArgs) { a.Value = "input" })
		check(err == nil, "middleware: %v", err)

		return []string{
			fmt.Sprintf("success:%s:%d:%t", structured.Value, structured.Count, structured.AgentAuthorized),
			supported,
			fmt.Sprintf("stream:%s:%d", stream.Stdout, stream.Result.Count),
			"middleware:" + middleware,
		}
	})

	unauthorized := Unauthorized.Implement(func(UnauthorizedID) *state { return &state{} })
	unauthorized.Handle(Denied, func(_ *golem.Context[state], _ golem.Unit) string {
		_, err := conformance.Structured.Call(func(a *conformance.StructuredArgs) { a.Value = "denied" })
		var denied *tool.CallError
		if errors.As(err, &denied) && denied.Kind == tool.CallDenied {
			return denied.Message
		}
		if err != nil {
			return "unexpected-error"
		}
		return "unexpected-success"
	})
}
