// Package rpccaller is the RPC caller: it invokes the ledger agent via typed
// cross-agent RPC, exercising the synchronous Call and the async CallAsync +
// Future.Get shapes. Both unwrap the ledger's Result and return the new total.
package rpccaller

import (
	"agent-sdk-go/ledger"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type Id struct{ Name string }
type State struct{}

var Agent = golem.DefineAgent[Id, State](
	golem.Spec{Name: "RpcAgent", Description: "Cross-agent RPC caller", Mode: golem.Durable},
	func(Id) *State { return &State{} },
)

type CallIn struct {
	Region string
	Amount int64
}

var (
	Call = golem.DefineMethod[Id, CallIn, int64]("call",
		golem.Desc("Record via a synchronous RPC and return the ledger's new total"))
	Async = golem.DefineMethod[Id, CallIn, int64]("async",
		golem.Desc("Record via CallAsync + Future.Get and return the ledger's new total"))
)

func init() {
	golem.Implement(Agent, Call, func(_ *golem.Context[State], in CallIn) int64 {
		c := golem.ClientFor(ledger.Agent, ledger.Id{Region: in.Region})
		res := ledger.Record.Call(c, ledger.RecordIn{Amount: in.Amount})
		if res.IsErr() {
			panic(res.Err())
		}
		return res.Ok()
	})
	golem.Implement(Agent, Async, func(_ *golem.Context[State], in CallIn) int64 {
		c := golem.ClientFor(ledger.Agent, ledger.Id{Region: in.Region})
		res := ledger.Record.CallAsync(c, ledger.RecordIn{Amount: in.Amount}).Get()
		if res.IsErr() {
			panic(res.Err())
		}
		return res.Ok()
	})
}
