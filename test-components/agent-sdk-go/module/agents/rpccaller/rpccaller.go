// Package rpccaller is the DEFINITION of the cross-agent RPC caller. The
// behaviour lives in rpccaller/impl.
package rpccaller

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type CallIn struct {
	Region string
	Amount int64
}

type CallParallelIn struct {
	Regions []string
	Amount  int64
}

type AwaitRemoteIn struct {
	Name     string
	OplogIdx int64
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "RpcAgent", Description: "Cross-agent RPC caller", Mode: golem.Durable,
})

var (
	Call  = Agent.Method[CallIn, int64]("call", golem.Description("Record via a synchronous RPC and return the ledger's new total"))
	Async = Agent.Method[CallIn, int64]("async", golem.Description("Record via CallAsync + Future.Get and return the ledger's new total"))
	// CallParallel records on every region's ledger at once, one goroutine per
	// region, and returns the totals in region order.
	CallParallel = Agent.Method[CallParallelIn, []int64]("call-parallel", golem.Description("Record on several ledgers concurrently from goroutines"))
	// AwaitRemote blocks on another agent's promise through a synchronous RPC,
	// so the caller stays inside the call for as long as the promise is open.
	AwaitRemote = Agent.Method[AwaitRemoteIn, string]("await-remote", golem.Description("Await a PromiseAgent's promise through a synchronous RPC"))
	// AwaitRemoteAsync is AwaitRemote through CallAsync + Future.Get.
	AwaitRemoteAsync = Agent.Method[AwaitRemoteIn, string]("await-remote-async", golem.Description("Await a PromiseAgent's promise through CallAsync + Future.Get"))
)
