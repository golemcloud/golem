// Package impl is the IMPLEMENTATION of the RPC caller: it invokes
// the ledger agent via typed cross-agent RPC (synchronous Call and async
// CallAsync + Future.Get), unwrapping the ledger's Result with MustOk. Importing
// only ledger (the callee's DEFINITION) keeps the two impls free of an
// import cycle.
package impl

import (
	"agent-sdk-go/agents/ledger"
	"agent-sdk-go/agents/rpccaller"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(rpccaller.Agent, func(rpccaller.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, rpccaller.Call, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		c := ledger.Agent.Get(ledger.Id{Region: in.Region})
		return ledger.Record.Call(c, ledger.RecordIn{Amount: in.Amount}).MustOk()
	})
	golem.Handle(agent, rpccaller.Async, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		c := ledger.Agent.Get(ledger.Id{Region: in.Region})
		return ledger.Record.CallAsync(c, ledger.RecordIn{Amount: in.Amount}).Get().MustOk()
	})
}
