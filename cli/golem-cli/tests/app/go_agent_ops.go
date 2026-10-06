// Package ops is the Go agent-management fixture driven by go_agent_ops.rs.
package ops

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/oplog"
	"github.com/golemcloud/golem/sdks/go/golem/retry"
)

type CounterID struct{ Name string }

var Counter = golem.DefineAgent[CounterID](golem.Spec{Name: "CounterAgent"})

var (
	Increment = Counter.Method[golem.Unit, int64]("increment")
	Value     = Counter.Method[golem.Unit, int64]("value")
)

type SourceID struct{ Name string }

var Source = golem.DefineAgent[SourceID](golem.Spec{Name: "SourceAgent"})

// Failing streams one item and then fails its production.
var Failing = Source.Method[golem.Unit, golem.AgentStream[int32]]("failing")

type SpendIn struct{ Token golem.QuotaToken }

// Spend uses one unit of the quota it is handed.
var Spend = Source.Method[SpendIn, string]("spend")

type CardIn struct{ Card golem.PermissionCard }

// Relay hands a permission card back; publishing it checks that the host
// accepts cards in agent method signatures.
var Relay = Source.Method[CardIn, golem.PermissionCard]("relay")

// Slow sleeps for two seconds.
var Slow = Source.Method[golem.Unit, string]("slow")

type sourceState struct{}

type GreeterID struct{ Name string }

type GreeterConfig struct {
	Greeting string
	Token    golem.Secret[string]
}

// Greeter is configured, so a reflected caller can override its configuration.
var Greeter = golem.DefineConfiguredAgent[GreeterID, GreeterConfig](golem.Spec{Name: "ConfiguredGreeter"})

var Greet = Greeter.Method[golem.Unit, string]("greet")

// Share hands its config secret to the caller, as a secret handle.
var Share = Greeter.Method[golem.Unit, golem.Secret[string]]("share")

type greeterState struct{ greeting, name string }

type OpsID struct{ Name string }

type NameIn struct{ Name string }

// BoundedIn carries a schema restriction, which the platform enforces before
// the method runs.
type BoundedIn struct {
	N uint32 `golem:"min=1,max=10"`
}

var Ops = golem.DefineAgent[OpsID](golem.Spec{Name: "OpsAgent"})

var (
	Self        = Ops.Method[golem.Unit, string]("self")
	RetryUntil3 = Ops.Method[NameIn, int64]("retryUntil3")
	ForkJoin    = Ops.Method[golem.Unit, string]("forkJoin")
	RevertOne   = Ops.Method[NameIn, int64]("revertOne")
	Counters    = Ops.Method[golem.Unit, string]("counters")
	Invocations = Ops.Method[golem.Unit, int64]("invocations")
	ReadFailing = Ops.Method[golem.Unit, string]("readFailing")
	Reflected   = Ops.Method[golem.Unit, string]("reflected")
	AwaitLate   = Ops.Method[golem.Unit, string]("awaitLate")
	SharedKey   = Ops.Method[golem.Unit, string]("sharedKey")
	TimerVsRPC  = Ops.Method[golem.Unit, string]("timerVsRpc")
	Sleepy      = Ops.Method[golem.Unit, string]("sleepy")
	Quota       = Ops.Method[golem.Unit, string]("quota")
	Bounded     = Ops.Method[BoundedIn, uint32]("bounded")
)

type counterState struct{ N int64 }

type opsState struct{}

func counterClient(name string) golem.Client[CounterID] { return Counter.Get(CounterID{Name: name}) }

func init() {
	counter := Counter.Implement(func(CounterID) *counterState { return &counterState{} })
	counter.Handle(Increment, func(ctx *golem.Context[counterState], _ golem.Unit) int64 {
		ctx.State.N++
		return ctx.State.N
	})
	counter.Handle(Value, func(ctx *golem.Context[counterState], _ golem.Unit) int64 { return ctx.State.N })

	source := Source.Implement(func(SourceID) *sourceState { return &sourceState{} })
	source.Handle(Failing, func(*golem.Context[sourceState], golem.Unit) golem.AgentStream[int32] {
		// The failure is deliberate; retrying it would only repeat it.
		retry.Set(retry.Named("stream-failure-test", retry.Never()).WithPriority(1 << 31))
		return golem.ProduceStream(func(w *golem.AgentStreamWriter[int32]) error {
			if err := w.Write(1); err != nil {
				return err
			}
			return fmt.Errorf("intentional producer failure")
		})
	})

	greeter := Greeter.ImplementConfigured(func(ctx *golem.InitContext[GreeterID, greeterState, GreeterConfig]) *greeterState {
		return &greeterState{greeting: ctx.Config().Greeting, name: ctx.ID().Name}
	})
	greeter.Handle(Share, func(ctx *golem.Context[greeterState], _ golem.Unit) golem.Secret[string] {
		return ctx.Config(Greeter).Token
	})
	greeter.Handle(Greet, func(ctx *golem.Context[greeterState], _ golem.Unit) string {
		return ctx.State.greeting + " " + ctx.State.name
	})

	source.Handle(Slow, func(*golem.Context[sourceState], golem.Unit) string {
		time.Sleep(2 * time.Second)
		return "slow-done"
	})

	source.Handle(Relay, func(_ *golem.Context[sourceState], in CardIn) golem.PermissionCard { return in.Card })

	source.Handle(Spend, func(_ *golem.Context[sourceState], in SpendIn) string {
		_, err := golem.WithReservation(in.Token, 1, func(*golem.Reservation) (uint64, golem.Unit) { return 1, golem.Unit{} })
		return fmt.Sprintf("child:%v", err)
	})

	ops := Ops.Implement(func(OpsID) *opsState { return &opsState{} })

	// The api-calls resource holds three units and rejects beyond them: two are
	// spent here, one by another agent through a split token.
	ops.Handle(Bounded, func(_ *golem.Context[opsState], in BoundedIn) uint32 { return in.N })

	ops.Handle(Quota, func(*golem.Context[opsState], golem.Unit) string {
		tok := golem.NewQuotaToken("api-calls", 1)
		for range 2 {
			r, err := tok.Reserve(1)
			if err != nil {
				return "reserve: " + err.Error()
			}
			r.Commit(1)
		}
		child := Spend.Call(Source.Get(SourceID{Name: "quota"}), SpendIn{Token: tok.Split(1)})
		_, err := tok.Reserve(1)
		var failed *golem.FailedReservation
		return fmt.Sprintf("%s|exhausted:%t", child, errors.As(err, &failed))
	})

	// A secret received from another agent is revealed through the host.
	ops.Handle(SharedKey, func(*golem.Context[opsState], golem.Unit) string {
		return "revealed:" + Share.Call(Greeter.Get(GreeterID{Name: "keeper"}), golem.Unit{}).Get()
	})

	ops.Handle(Sleepy, func(*golem.Context[opsState], golem.Unit) string {
		start := time.Now()
		time.Sleep(300 * time.Millisecond)
		return fmt.Sprintf("slept:%t", time.Since(start) >= 300*time.Millisecond)
	})

	ops.Handle(TimerVsRPC, func(*golem.Context[opsState], golem.Unit) string {
		start := time.Now()
		done := make(chan string, 1)
		fut := Slow.CallAsync(Source.Get(SourceID{Name: "slow"}), golem.Unit{})
		go func() { done <- fut.Get() }()
		select {
		case r := <-done:
			return fmt.Sprintf("rpc-first:%s:%dms", r, time.Since(start).Milliseconds())
		case <-time.After(200 * time.Millisecond):
			return fmt.Sprintf("timer-first:%dms", time.Since(start).Milliseconds())
		}
	})

	// A wait on a promise gives up at its deadline; completing the promise
	// afterwards still delivers to a new wait.
	ops.Handle(AwaitLate, func(*golem.Context[opsState], golem.Unit) string {
		p := golem.NewPromise[string]()
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		_, err := p.AwaitContext(ctx)
		if !errors.Is(err, context.DeadlineExceeded) {
			return fmt.Sprintf("the wait ended with %v", err)
		}
		golem.CompletePromise(p.ID(), "late")
		return "timed-out|" + p.Await()
	})

	// A reflected caller creates a configured agent with an override, calls it
	// both ways, and is refused an undeclared configuration path locally.
	ops.Handle(Reflected, func(*golem.Context[opsState], golem.Unit) string {
		t, found := golem.DiscoverAgentType("ConfiguredGreeter")
		if !found {
			return "not discovered"
		}
		_, err := t.Get(map[string]any{"name": "x"}, golem.WithConfigJSON([]string{"greting"}, "typo"))
		if err == nil {
			return "an undeclared path was accepted"
		}
		c, err := t.Get(map[string]any{"name": "r"}, golem.WithConfigJSON([]string{"greeting"}, "hej"))
		if err != nil {
			return "get: " + err.Error()
		}
		called, _, err := c.Call("greet", map[string]any{})
		if err != nil {
			return "call: " + err.Error()
		}
		pending, err := c.CallAsync("greet", map[string]any{})
		if err != nil {
			return "callAsync: " + err.Error()
		}
		awaited, err := pending.Wait()
		if err != nil {
			return "wait: " + err.Error()
		}
		return fmt.Sprintf("%v|%v|%t", called, awaited, pending.ID.AgentID == c.AgentID())
	})

	// A failed production must not reach the reader as a clean end.
	ops.Handle(ReadFailing, func(*golem.Context[opsState], golem.Unit) string {
		items, err := Failing.Call(Source.Get(SourceID{Name: "s"}), golem.Unit{}).Collect()
		if err == nil {
			return fmt.Sprintf("clean-eof:%v", items)
		}
		return fmt.Sprintf("failed:%v", err)
	})

	ops.Handle(Self, func(*golem.Context[opsState], golem.Unit) string {
		md := golem.MustGetSelfMetadata()
		return fmt.Sprintf("%s %s", md.AgentID.AgentID, md.Status)
	})

	// Each revert rewinds this agent, but not the counter it calls, so the
	// retries converge.
	ops.Handle(RetryUntil3, func(_ *golem.Context[opsState], in NameIn) int64 {
		cp := golem.NewCheckpoint()
		n := Increment.Call(counterClient(in.Name), golem.Unit{})
		cp.AssertOrRevert(n >= 3)
		return n
	})

	ops.Handle(ForkJoin, func(*golem.Context[opsState], golem.Unit) string {
		result := golem.NewPromise[string]()
		forked, phantom := golem.MustFork()
		if forked {
			golem.CompletePromise(result.ID(), "from-fork")
			return "fork"
		}
		if phantom == (golem.UUID{}) {
			panic("fork returned no phantom id")
		}
		return "original+" + result.Await()
	})

	ops.Handle(RevertOne, func(_ *golem.Context[opsState], in NameIn) int64 {
		c := counterClient(in.Name)
		Increment.Call(c, golem.Unit{})
		Increment.Call(c, golem.Unit{})
		id, ok := golem.ResolveAgentIDStrict("go-agent-ops:main", c.AgentID())
		if !ok {
			panic("the counter agent did not resolve")
		}
		golem.MustRevertAgent(id, golem.RevertLastInvocations(1))
		return Value.Call(c, golem.Unit{})
	})

	ops.Handle(Counters, func(*golem.Context[opsState], golem.Unit) string {
		self := golem.MustGetSelfMetadata()
		var names []string
		for md := range golem.MustGetAgents(self.AgentID.ComponentID, golem.GetAgentsOptions{
			Filter: golem.AgentNameFilter(golem.StringFilterStartsWith, "CounterAgent"),
		}) {
			names = append(names, md.AgentID.AgentID)
		}
		return strings.Join(names, ",")
	})

	// Reading a whole oplog lowers large pages into guest memory, which can
	// finish a GC cycle inside cabi_realloc.
	ops.Handle(Invocations, func(*golem.Context[opsState], golem.Unit) int64 {
		var n int64
		for e, err := range oplog.Get(golem.MustGetSelfMetadata().AgentID, 0) {
			if err != nil {
				panic(err)
			}
			if e.Tag() == oplog.AgentInvocationStarted {
				n++
			}
		}
		return n
	})
}
