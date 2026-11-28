// Package impl is the IMPLEMENTATION of the RPC caller: it invokes
// the ledger agent via typed cross-agent RPC (an awaited Call, and Trigger
// followed by the handle's AwaitResult), unwrapping the ledger's Result with MustOk. Importing
// only ledger (the callee's DEFINITION) keeps the two impls free of an
// import cycle.
package impl

import (
	"errors"
	"runtime"
	"sync"
	"time"

	"agent-sdk-go/agents/counter"
	"agent-sdk-go/agents/ledger"
	"agent-sdk-go/agents/promises"
	"agent-sdk-go/agents/rpccaller"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = rpccaller.Agent.Implement(func(rpccaller.Id) *state { return &state{} })

// errorKind names the kind of a failed remote call, so a test asserts the kind
// and not a message.
func errorKind(err error) string {
	var rce *golem.RemoteCallError
	if errors.As(err, &rce) {
		return rce.Kind.String()
	}
	return "unexpected:" + err.Error()
}

// CollectedHandle runs a garbage collection every collectionPoll, at most
// collectionAttempts times, until the abandoned handle is collected.
const (
	collectionPoll     = 50 * time.Millisecond
	collectionAttempts = 200
)

// heldHandle keeps an invocation handle reachable until release, and reports
// through collected when the garbage collector has released it.
type heldHandle struct {
	inv       *golem.Invocation[string]
	collected chan struct{}
}

func holdHandle(inv *golem.Invocation[string]) *heldHandle {
	collected := make(chan struct{})
	runtime.AddCleanup(inv, func(ch chan struct{}) { close(ch) }, collected)
	return &heldHandle{inv: inv, collected: collected}
}

func (h *heldHandle) release() { h.inv = nil }

func (h *heldHandle) isCollected() bool {
	select {
	case <-h.collected:
		return true
	default:
		return false
	}
}

func init() {
	agent.Handle(rpccaller.Call, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		c := ledger.Agent.Upsert(ledger.Id{Region: in.Region})
		return ledger.Record.MustCall(c, ledger.RecordIn{Amount: in.Amount}).MustOk()
	})
	agent.Handle(rpccaller.AwaitRemote, func(_ *golem.Context[state], in rpccaller.AwaitRemoteIn) string {
		c := promises.Agent.Upsert(promises.Id{Name: in.Name})
		return promises.Await.MustCall(c, promises.OplogIdxIn{OplogIdx: in.OplogIdx})
	})
	agent.Handle(rpccaller.AwaitRemoteTrigger, func(_ *golem.Context[state], in rpccaller.AwaitRemoteIn) string {
		c := promises.Agent.Upsert(promises.Id{Name: in.Name})
		inv := promises.Await.MustTrigger(c, promises.OplogIdxIn{OplogIdx: in.OplogIdx})
		return golem.Must(inv.AwaitResult())
	})
	agent.Handle(rpccaller.CallParallel, func(_ *golem.Context[state], in rpccaller.CallParallelIn) []int64 {
		totals := make([]int64, len(in.Regions))
		var wg sync.WaitGroup
		for i, region := range in.Regions {
			wg.Go(func() {
				c := ledger.Agent.Upsert(ledger.Id{Region: region})
				totals[i] = ledger.Record.MustCall(c, ledger.RecordIn{Amount: in.Amount}).MustOk()
			})
		}
		wg.Wait()
		return totals
	})
	agent.Handle(rpccaller.TriggerAwait, func(_ *golem.Context[state], in rpccaller.CallIn) int64 {
		c := ledger.Agent.Upsert(ledger.Id{Region: in.Region})
		inv := ledger.Record.MustTrigger(c, ledger.RecordIn{Amount: in.Amount})
		return golem.Must(inv.AwaitResult()).MustOk()
	})
	agent.Handle(rpccaller.InvocationHandles, func(_ *golem.Context[state], in rpccaller.HandlesIn) rpccaller.HandlesReport {
		c := counter.Agent.Upsert(counter.CounterID{Name: in.Counter})
		var report rpccaller.HandlesReport

		triggered := counter.Add.MustTrigger(c, counter.AddIn{By: 3})
		golem.Must(triggered.AwaitResult())
		report.TriggeredStatus = golem.Must(triggered.Status()).String()
		report.CancelCompleted = golem.Must(triggered.Cancel())

		read := counter.Value.MustTrigger(c, golem.Unit{})
		report.RebuiltValue = golem.Must(counter.Value.Invocation(read.ID()).AwaitResult())

		scheduled := counter.Add.MustSchedule(c, time.Now().Add(time.Hour), counter.AddIn{By: 100})
		report.ScheduledStatus = golem.Must(scheduled.Status()).String()
		report.CancelScheduled = golem.Must(scheduled.Cancel())
		report.CancelledStatus = golem.Must(scheduled.Status()).String()
		if _, err := scheduled.AwaitResult(); err != nil {
			report.AwaitCancelled = errorKind(err)
		} else {
			report.AwaitCancelled = "unexpected:completed"
		}

		report.Value = counter.Value.MustCall(c, golem.Unit{})
		return report
	})
	agent.Handle(rpccaller.CancelRunning, func(_ *golem.Context[state], in rpccaller.AwaitRemoteIn) rpccaller.CancelRunningReport {
		c := promises.Agent.Upsert(promises.Id{Name: in.Name})
		inv := promises.Await.MustTrigger(c, promises.OplogIdxIn{OplogIdx: in.OplogIdx})
		var report rpccaller.CancelRunningReport

		status := golem.Must(inv.Status())
		for attempt := 0; attempt < 300 && status != golem.InvocationRunning && status != golem.InvocationWaiting; attempt++ {
			time.Sleep(100 * time.Millisecond)
			status = golem.Must(inv.Status())
		}
		report.RunningStatus = status.String()

		if cancelled, err := inv.Cancel(); err != nil {
			report.CancelRunning = errorKind(err)
		} else if cancelled {
			report.CancelRunning = "unexpected:withdrawn"
		} else {
			report.CancelRunning = "unexpected:not-pending"
		}
		return report
	})
	agent.Handle(rpccaller.CollectedHandle, func(_ *golem.Context[state], in rpccaller.AwaitRemoteIn) rpccaller.CollectedHandleReport {
		c := promises.Agent.Upsert(promises.Id{Name: in.Name})
		first := holdHandle(promises.Await.MustTrigger(c, promises.OplogIdxIn{OplogIdx: in.OplogIdx}))
		second := promises.Await.Invocation(first.inv.ID())

		type awaited struct {
			value string
			err   error
		}
		done := make(chan awaited, 1)
		go func() {
			value, err := second.AwaitResult()
			done <- awaited{value, err}
		}()
		// Lets the goroutine issue its await before the first handle becomes
		// unreachable.
		runtime.Gosched()
		first.release()

		for attempt := 0; attempt < collectionAttempts && !first.isCollected(); attempt++ {
			runtime.GC()
			time.Sleep(collectionPoll)
		}
		// One more collection most likely runs the cleanup that drops the first
		// handle's resource too: Go does not order it after the marker's cleanup,
		// and the drop writes nothing the test can observe.
		runtime.GC()
		runtime.Gosched()

		var result awaited
		pending := false
		select {
		case result = <-done:
		default:
			pending = true
		}
		report := rpccaller.CollectedHandleReport{Collected: first.isCollected(), PendingAtCollection: pending}
		// The test completes the promise only after this status call.
		golem.Must(second.Status())
		if pending {
			result = <-done
		}

		if result.err != nil {
			report.Value = "unexpected:" + errorKind(result.err)
		} else {
			report.Value = result.value
		}
		report.FinalStatus = golem.Must(second.Status()).String()
		return report
	})
}
