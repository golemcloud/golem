// Package ops is the Go agent-management fixture driven by go_agent_ops.rs.
package ops

import (
	"fmt"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type CounterID struct{ Name string }

var Counter = golem.DefineAgent[CounterID](golem.Spec{Name: "CounterAgent"})

var (
	Increment = Counter.Method[golem.Unit, int64]("increment")
	Value     = Counter.Method[golem.Unit, int64]("value")
)

type OpsID struct{ Name string }

type NameIn struct{ Name string }

var Ops = golem.DefineAgent[OpsID](golem.Spec{Name: "OpsAgent"})

var (
	Self        = Ops.Method[golem.Unit, string]("self")
	RetryUntil3 = Ops.Method[NameIn, int64]("retryUntil3")
	ForkJoin    = Ops.Method[golem.Unit, string]("forkJoin")
	RevertOne   = Ops.Method[NameIn, int64]("revertOne")
	Counters    = Ops.Method[golem.Unit, string]("counters")
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

	ops := Ops.Implement(func(OpsID) *opsState { return &opsState{} })

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
}
