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

type HandlesIn struct{ Counter string }

// HandlesReport is what InvocationHandles observed. Statuses and error kinds
// are their Go names (InvocationStatus.String, RemoteCallErrorKind.String).
type HandlesReport struct {
	TriggeredStatus string
	CancelCompleted bool
	RebuiltValue    int64
	ScheduledStatus string
	CancelScheduled bool
	CancelledStatus string
	AwaitCancelled  string
	Value           int64
}

// CollectedHandleReport is what CollectedHandle observed.
type CollectedHandleReport struct {
	Collected bool
	// PendingAtCollection is true when the other handle's await had not
	// returned yet when the first handle was collected.
	PendingAtCollection bool
	Value               string
	FinalStatus         string
}

// CancelRunningReport is what CancelRunning observed.
type CancelRunningReport struct {
	RunningStatus string
	CancelRunning string
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "RpcAgent", Description: "Cross-agent RPC caller", Mode: golem.Durable,
})

var (
	Call = Agent.Method[CallIn, int64]("call", golem.Description("Record via a synchronous RPC and return the ledger's new total"))
	// TriggerAwait records through Trigger and the handle's AwaitResult.
	TriggerAwait = Agent.Method[CallIn, int64]("trigger-await", golem.Description("Record via Trigger + Invocation.AwaitResult and return the ledger's new total"))
	// CallParallel records on every region's ledger at once, one goroutine per
	// region, and returns the totals in region order.
	CallParallel = Agent.Method[CallParallelIn, []int64]("call-parallel", golem.Description("Record on several ledgers concurrently from goroutines"))
	// AwaitRemote blocks on another agent's promise through a synchronous RPC,
	// so the caller stays inside the call for as long as the promise is open.
	AwaitRemote = Agent.Method[AwaitRemoteIn, string]("await-remote", golem.Description("Await a PromiseAgent's promise through a synchronous RPC"))
	// AwaitRemoteTrigger is AwaitRemote through Trigger + Invocation.AwaitResult.
	AwaitRemoteTrigger = Agent.Method[AwaitRemoteIn, string]("await-remote-trigger", golem.Description("Await a PromiseAgent's promise through Trigger + Invocation.AwaitResult"))
	// InvocationHandles triggers and schedules calls on a counter, then reads,
	// awaits, cancels and rebuilds their handles.
	InvocationHandles = Agent.Method[HandlesIn, HandlesReport]("invocation-handles", golem.Description("Exercise the trigger and schedule invocation handles on a counter"))
	// CancelRunning triggers a PromiseAgent await, waits until it has started,
	// and tries to cancel it. The await keeps running until its promise is
	// completed.
	CancelRunning = Agent.Method[AwaitRemoteIn, CancelRunningReport]("cancel-running", golem.Description("Try to cancel a started PromiseAgent await"))
	// CollectedHandle triggers a PromiseAgent await, awaits it through a second
	// handle built from its ID, and lets garbage collections release the first
	// handle while that await waits for the promise. Once the first handle is
	// collected it reads the second handle's status, which tells the test to
	// complete the promise.
	CollectedHandle = Agent.Method[AwaitRemoteIn, CollectedHandleReport]("collected-handle", golem.Description("Await an invocation while the garbage collector releases another handle of it"))
)
