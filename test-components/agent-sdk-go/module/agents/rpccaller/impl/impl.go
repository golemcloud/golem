// Package impl is the IMPLEMENTATION of the RPC caller: it invokes
// the ledger agent via typed cross-agent RPC (synchronous Call and async
// CallAsync + Future.Get), unwrapping the ledger's Result with MustOk. Importing
// only ledger (the callee's DEFINITION) keeps the two impls free of an
// import cycle.
package impl

import (
	"sync"

	"agent-sdk-go/agents/ledger"
	"agent-sdk-go/agents/promises"
	"agent-sdk-go/agents/rpccaller"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durability"
)

type state struct{}

var agent = rpccaller.Agent.Implement(func(rpccaller.Id) *state { return &state{} })

func init() {
	agent.Handle(rpccaller.Call, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		c := ledger.Agent.Get(ledger.Id{Region: in.Region})
		return ledger.Record.Call(c, ledger.RecordIn{Amount: in.Amount}).MustOk()
	})
	agent.Handle(rpccaller.AtomicCall, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		var total int64
		durability.Atomically(func() {
			c := ledger.Agent.Get(ledger.Id{Region: in.Region})
			total = ledger.Record.Call(c, ledger.RecordIn{Amount: in.Amount}).MustOk()
		})
		return total
	})
	agent.Handle(rpccaller.AwaitRemote, func(_ *golem.Context[state], in rpccaller.AwaitRemoteIn) string {
		c := promises.Agent.Get(promises.Id{Name: in.Name})
		return promises.Await.Call(c, promises.OplogIdxIn{OplogIdx: in.OplogIdx})
	})
	agent.Handle(rpccaller.AwaitRemoteAsync, func(_ *golem.Context[state], in rpccaller.AwaitRemoteIn) string {
		c := promises.Agent.Get(promises.Id{Name: in.Name})
		return promises.Await.CallAsync(c, promises.OplogIdxIn{OplogIdx: in.OplogIdx}).Get()
	})
	agent.Handle(rpccaller.CallParallel, func(_ *golem.Context[state], in rpccaller.CallParallelIn) []int64 {
		totals := make([]int64, len(in.Regions))
		var wg sync.WaitGroup
		for i, region := range in.Regions {
			wg.Go(func() {
				c := ledger.Agent.Get(ledger.Id{Region: region})
				totals[i] = ledger.Record.Call(c, ledger.RecordIn{Amount: in.Amount}).MustOk()
			})
		}
		wg.Wait()
		return totals
	})
	agent.Handle(rpccaller.Async, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		c := ledger.Agent.Get(ledger.Id{Region: in.Region})
		return ledger.Record.CallAsync(c, ledger.RecordIn{Amount: in.Amount}).Get().MustOk()
	})
}
