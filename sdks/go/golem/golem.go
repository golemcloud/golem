// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Package golem is the Golem Go SDK for authoring agents.
//
// An agent's DEFINITION (its contract) and its IMPLEMENTATION (its behaviour)
// live in separate packages, so agents can call one another without Go import
// cycles. The definition is state-free and is what other agents import:
//
//	// package counteragent — the definition (no state)
//	type CounterId struct{ Name string } // constructor params; also the type-level identity
//	type AddIn struct{ By int64 }
//
//	var Agent = golem.DefineAgent[CounterId](golem.Spec{Name: "CounterAgent"})
//	var (
//	    Add   = golem.DefineMethod[CounterId, AddIn, int64]("add")
//	    Value = golem.DefineMethod[CounterId, golem.Unit, int64]("value")
//	)
//
// The implementation lives in its own package and owns the private state:
//
//	// package counteragentimpl — the behaviour (state is unexported)
//	type state struct{ count int64 }
//
//	var counter = golem.Implement(counteragent.Agent, func(counteragent.CounterId) *state { return &state{} })
//
//	func init() {
//	    golem.Handle(counter, counteragent.Add, func(ctx *golem.Context[state], in counteragent.AddIn) int64 {
//	        ctx.State.count += in.By
//	        return ctx.State.count
//	    })
//	    golem.Handle(counter, counteragent.Value, func(ctx *golem.Context[state], _ golem.Unit) int64 {
//	        return ctx.State.count
//	    })
//	}
//
// Another agent calls it through the definition — [AgentDefinition.Get] returns a
// client, and the method descriptor carries the call:
//
//	client := counteragent.Agent.Get(counteragent.CounterId{Name: "c1"})
//	n := counteragent.Add.Call(client, counteragent.AddIn{By: 5})
//
// Method descriptors are package-level values so the same value drives the
// agent-type schema, the implementation binding, and typed calls from other
// agents. Schemas are derived from the Go types by reflection; there is no code
// generation step.
//
// A component's main package blank-imports each implementation package (which
// registers its agent on import) and this package (which links the generated
// golem:agent/guest export glue into the component):
//
//	import (
//	    _ "example.com/app/counteragentimpl"
//	    _ "github.com/golemcloud/golem/sdks/go/golem"
//	)
package golem

import (
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"

	// Links the generated wasmexport glue into the component.
	_ "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wit_exports"
)

// Unit is the empty input/output marker. Go has no arity overloading, so a
// method with no parameters takes Unit, and one with no result returns Unit.
type Unit struct{}

// Mode selects an agent's durability model.
type Mode uint8

const (
	// Durable agents persist their state and replay deterministically.
	Durable Mode = iota
	// Ephemeral agents do not persist state across invocations.
	Ephemeral
)

func (m Mode) toWit() common.AgentMode {
	if m == Ephemeral {
		return common.AgentModeEphemeral
	}
	return common.AgentModeDurable
}

// SnapshotPolicy declares whether and how often the platform snapshots an
// agent's state. The zero value is [SnapshotDisabled]. Build one with
// [SnapshotDefault], [SnapshotPeriodic], or [SnapshotEveryN].
//
// A snapshot serializes the agent's state: if the state implements
// [Snapshotter], that is used; otherwise the exported fields are JSON-encoded
// (see [Snapshotter] for the caveat).
type SnapshotPolicy struct {
	kind   snapshotKind
	amount uint64 // periodic: nanoseconds; every-n: count
}

type snapshotKind uint8

const (
	snapDisabled snapshotKind = iota
	snapDefault
	snapPeriodic
	snapEveryN
)

// SnapshotDisabled is the zero-value policy: the platform never snapshots.
var SnapshotDisabled = SnapshotPolicy{kind: snapDisabled}

// SnapshotDefault enables snapshotting at the platform's default cadence.
var SnapshotDefault = SnapshotPolicy{kind: snapDefault}

// SnapshotPeriodic snapshots on a fixed time interval.
func SnapshotPeriodic(d time.Duration) SnapshotPolicy {
	return SnapshotPolicy{kind: snapPeriodic, amount: uint64(d.Nanoseconds())}
}

// SnapshotEveryN snapshots every n invocations.
func SnapshotEveryN(n uint16) SnapshotPolicy {
	return SnapshotPolicy{kind: snapEveryN, amount: uint64(n)}
}

func (p SnapshotPolicy) toWit() common.Snapshotting {
	switch p.kind {
	case snapDefault:
		return common.MakeSnapshottingEnabled(common.MakeSnapshottingConfigDefault())
	case snapPeriodic:
		return common.MakeSnapshottingEnabled(common.MakeSnapshottingConfigPeriodic(p.amount))
	case snapEveryN:
		return common.MakeSnapshottingEnabled(common.MakeSnapshottingConfigEveryNInvocation(uint16(p.amount)))
	default:
		return common.MakeSnapshottingDisabled()
	}
}

// Snapshotter lets an agent's state control its own snapshot serialization.
// Implement it on the state type (usually with a pointer receiver) when the
// default reflective snapshot is not enough — in particular, Go reflection
// cannot see unexported fields, so the idiomatic private state (e.g. an
// unexported `count`) is only captured through a Snapshotter. Without one, the
// snapshot is the JSON of the state's exported fields.
type Snapshotter interface {
	Save() ([]byte, error)
	Load([]byte) error
}

// Spec is the declarative part of an agent definition.
type Spec struct {
	// Name is the wire-level agent type name (as seen by the platform and by
	// other agents). Required.
	Name string
	// Description is surfaced in the agent type metadata.
	Description string
	// Mode defaults to Durable.
	Mode Mode
	// HTTP, when set, mounts the agent's methods under an HTTP path prefix so the
	// platform can route requests to them. The prefix binds the Id fields; see
	// [Mount] and attach per-method routes with [HTTP].
	HTTP *Mount
	// Snapshot sets the agent's snapshot policy; the zero value is
	// [SnapshotDisabled]. See [SnapshotPolicy] and [Snapshotter].
	Snapshot SnapshotPolicy
}

// Context is passed to every method handler. State is the agent instance's
// private state, as returned by the agent's init function.
type Context[S any] struct {
	State   *S
	agentID string
}

// AgentID returns the raw agent id the running instance was initialized with.
func (c *Context[S]) AgentID() string { return c.agentID }

// agentScope is the unexported capability carried by a method's *[Context].
// [Config] requires it, so config can be read only from inside a running
// method. The *S in the signature ties the scope to the agent's state type, so
// one agent cannot read another agent's config (whose state type differs) at
// compile time. (A constructor reads config off its own *[InitContext] via
// [InitContext.Config], so it is not an agentScope.)
type agentScope[S any] interface {
	agentScopeState() *S
}

// agentScopeState satisfies [agentScope] for a method context. It exists only to
// gate [Config] at compile time.
//
//nolint:unused // false positive: staticcheck's unused can't trace generic-interface satisfaction; the compiler requires this method for agentScope[S].
func (c *Context[S]) agentScopeState() *S { return c.State }

// InitContext is the execution scope passed to a [DefineConfiguredAgent]
// constructor. It carries the constructor parameters ([InitContext.ID]) and reads
// the agent's config with [InitContext.Config]. Its Cfg type parameter is what
// lets Config return the agent's own config type without a type parameter on the
// method. Agents declared with the plain [DefineAgent] receive their id directly
// and never see this type.
type InitContext[Id any, S any, Cfg any] struct {
	id      Id
	agentID string
}

// ID returns the constructor parameters the agent instance was created with.
func (c *InitContext[Id, S, Cfg]) ID() Id { return c.id }

// AgentID returns the raw agent id the instance is being initialized with.
func (c *InitContext[Id, S, Cfg]) AgentID() string { return c.agentID }

// AgentDefinition is the state-free handle returned by [DefineAgent] /
// [DefineConfiguredAgent]. Id is the constructor parameter type, which doubles as
// the agent's type-level identity; Cfg is the agent's config struct ([NoConfig]
// for a config-less agent). It carries no state type: callers obtain a client
// with [AgentDefinition.Get] and never depend on how the target stores state.
// The behaviour (and the private state) is attached separately with [Implement].
type AgentDefinition[Id any, Cfg any] struct{ name string }

// Name returns the agent's wire-level type name.
func (a *AgentDefinition[Id, Cfg]) Name() string { return a.name }

// AgentImpl is the state-bound implementation handle returned by [Implement] /
// [ImplementConfigured]. Register the agent's methods on it with [Handle]. Its S
// type parameter is the agent's private state type, introduced at [Implement]; it
// is used only inside the agent's implementation package (callers use the
// [AgentDefinition] instead).
type AgentImpl[Id any, S any, Cfg any] struct {
	d *definitions
	e *agentEntry
}

// MethodDef is a typed method descriptor: the single source of truth shared by
// the agent-type schema, the implementation binding, and calls from other
// agents. Id is the target agent's identity type, so a descriptor cannot be
// used against another agent's client.
type MethodDef[Id any, In any, Out any] struct {
	name          string
	desc          string
	descCount     int // how many times Desc was set; validated at Implement time
	endpoints     []Endpoint
	readOnly      *readOnlyConfig // non-nil => method is read-only; validated at Implement
	readOnlyCount int             // how many times ReadOnly was set
	cacheCount    int             // how many CachePolicy args were passed to ReadOnly
}

// Name returns the method's wire-level name.
func (m MethodDef[Id, In, Out]) Name() string { return m.name }

// MethodOpt configures a method descriptor.
type MethodOpt func(*methodOpts)

type methodOpts struct {
	desc          string
	descCount     int
	endpoints     []Endpoint
	readOnly      *readOnlyConfig
	readOnlyCount int
	cacheCount    int
}

// Desc sets a method's description, surfaced in the agent type metadata. Setting
// it more than once for the same method is a definition error, not a silent
// overwrite.
func Desc(s string) MethodOpt {
	return func(o *methodOpts) { o.desc = s; o.descCount++ }
}

// HTTP exposes a method over HTTP at one or more routes (the agent must also
// declare a [Mount] on its [Spec]). Grouping the endpoints in a single option
// keeps each route's bindings together:
//
//	golem.DefineMethod[CounterID, AddIn, int64]("add",
//	    golem.HTTP(golem.POST("/add?by={by}"), golem.GET("/add/{by}")))
func HTTP(endpoints ...Endpoint) MethodOpt {
	return func(o *methodOpts) { o.endpoints = append(o.endpoints, endpoints...) }
}

// CachePolicy controls how a read-only method's result is cached. Build one with
// [NoCache], [CacheUntilWrite], or [CacheFor], and pass it to [ReadOnly]. Its zero
// value is CacheUntilWrite.
type CachePolicy struct {
	kind cacheKind
	ttl  time.Duration // for cacheTTL
}

type cacheKind uint8

const (
	cacheUntilWrite cacheKind = iota // zero value = the default policy
	cacheNone
	cacheTTL
)

// NoCache never caches the method's result (the method still runs read-only).
func NoCache() CachePolicy { return CachePolicy{kind: cacheNone} }

// CacheUntilWrite caches the result until any mutating (non-read-only) method of
// the agent completes. It is the default policy for [ReadOnly].
func CacheUntilWrite() CachePolicy { return CachePolicy{kind: cacheUntilWrite} }

// CacheFor caches the result for the given duration. ttl must be positive; a
// zero/negative ttl is a definition error (use [NoCache] to disable caching).
func CacheFor(ttl time.Duration) CachePolicy { return CachePolicy{kind: cacheTTL, ttl: ttl} }

func (c CachePolicy) toWit() common.CachePolicy {
	switch c.kind {
	case cacheNone:
		return common.MakeCachePolicyNoCache()
	case cacheTTL:
		return common.MakeCachePolicyTtl(uint64(c.ttl.Nanoseconds()))
	default:
		return common.MakeCachePolicyUntilWrite()
	}
}

type readOnlyConfig struct {
	policy CachePolicy
}

// ReadOnly marks a method read-only: it must not perform remote side effects —
// the runtime traps outgoing HTTP and agent RPC from a read-only method — and in
// return its result is cached per the [CachePolicy]. With no argument the policy
// is [CacheUntilWrite]; pass at most one policy. Read-only is only valid on
// Durable agents (an [Ephemeral] agent has no shared state to read); violating
// that, passing more than one policy, or setting ReadOnly more than once is a
// definition error reported at discovery.
//
//	golem.DefineMethod[ShopId, golem.Unit, Catalog]("catalog", golem.ReadOnly())
//	golem.DefineMethod[ShopId, golem.Unit, Rates]("rates", golem.ReadOnly(golem.CacheFor(time.Minute)))
func ReadOnly(cache ...CachePolicy) MethodOpt {
	return func(o *methodOpts) {
		o.readOnlyCount++
		o.cacheCount += len(cache)
		rc := &readOnlyConfig{policy: CacheUntilWrite()}
		if len(cache) >= 1 {
			rc.policy = cache[0]
		}
		o.readOnly = rc
	}
}
