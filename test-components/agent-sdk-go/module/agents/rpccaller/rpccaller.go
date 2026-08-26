// Package rpccaller is the DEFINITION of the cross-agent RPC caller. The
// behaviour lives in rpccaller/impl.
package rpccaller

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type CallIn struct {
	Region string
	Amount int64
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "RpcAgent", Description: "Cross-agent RPC caller", Mode: golem.Durable,
})

var (
	Call = golem.DefineMethod[Id, CallIn, int64]("call",
		golem.Desc("Record via a synchronous RPC and return the ledger's new total"))
	Async = golem.DefineMethod[Id, CallIn, int64]("async",
		golem.Desc("Record via CallAsync + Future.Get and return the ledger's new total"))
	// AtomicCall makes the same RPC inside golem.Atomically — checks whether a
	// cross-agent call settles before an atomic region closes.
	AtomicCall = golem.DefineMethod[Id, CallIn, int64]("atomic-call",
		golem.Desc("Record via a synchronous RPC inside an atomic region"))
)
