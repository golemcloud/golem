// Package rpccalleragentimpl is the IMPLEMENTATION of the RPC caller: it invokes
// the ledger agent via typed cross-agent RPC (synchronous Call and async
// CallAsync + Future.Get), unwrapping the ledger's Result with MustOk. Importing
// only ledgeragent (the callee's DEFINITION) keeps the two impls free of an
// import cycle.
package rpccalleragentimpl

import (
	"agent-sdk-go/ledgeragent"
	"agent-sdk-go/rpccalleragent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var caller = golem.Implement(rpccalleragent.Agent, func(rpccalleragent.Id) *state { return &state{} })

func init() {
	golem.Handle(caller, rpccalleragent.Call, func(_ *golem.Context[state], in rpccalleragent.CallIn) int64 {
		c := ledgeragent.Agent.Get(ledgeragent.Id{Region: in.Region})
		return ledgeragent.Record.Call(c, ledgeragent.RecordIn{Amount: in.Amount}).MustOk()
	})
	golem.Handle(caller, rpccalleragent.Async, func(_ *golem.Context[state], in rpccalleragent.CallIn) int64 {
		c := ledgeragent.Agent.Get(ledgeragent.Id{Region: in.Region})
		return ledgeragent.Record.CallAsync(c, ledgeragent.RecordIn{Amount: in.Amount}).Get().MustOk()
	})
}
