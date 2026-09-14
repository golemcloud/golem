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

package golem

import (
	"reflect"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// fieldInfo is one exported struct field, in declaration order. Declaration
// order is the wire order: value trees encode records positionally.
type fieldInfo struct {
	name  string
	index int
	typ   reflect.Type
	codec *codec
}

type methodEntry struct {
	name      string
	desc      string
	inFields  []fieldInfo
	endpoints []Endpoint      // HTTP routes, if any
	readOnly  *readOnlyConfig // non-nil => read-only method with a cache policy
	outCodec  *codec          // nil => unit output
	// invoke is the erased dispatcher produced by Implement. Calling it is a
	// direct func-value call: no reflection is used to reach the handler.
	invoke func(state any, agentID string, in types.SchemaValueTree) (out *types.SchemaValueTree, err error)
}

type agentEntry struct {
	name     string
	desc     string
	mode     common.AgentMode
	mount    *Mount         // HTTP mount, if any
	snapshot SnapshotPolicy // snapshot cadence
	configs  []configDecl   // declared config keys + secrets
	idType   reflect.Type
	idFields []fieldInfo
	newState func(idVal reflect.Value, agentID string) any
	methods  map[string]*methodEntry
	order    []string
}

// instance is the single agent instance this worker runs. A component may define
// many agent types, but a worker is initialized as exactly one of them — so the
// live state is held here rather than on the (shared, immutable) type entry.
type instance struct {
	def     *agentEntry
	state   any
	agentID string
	// config is the agent's materialized config (its Cfg value), cached for the
	// worker's life on first read. Its local fields are read from the host once;
	// its secret fields are lazy handles that re-read on Secret.Get(). nil until
	// the first Config/InitContext.Config call.
	config any
}

// active is the running instance this worker was initialized as, or nil before
// initialize() has run. Unlike the definition state (see [definitions]) this is
// per-worker runtime state, so it stays a standalone package var.
var active *instance

// structFields returns the exported fields of a struct type in declaration
// order. Non-structs (e.g. Unit) yield no fields.
func (d *definitions) structFields(t reflect.Type) []fieldInfo {
	var out []fieldInfo
	if t == nil || t.Kind() != reflect.Struct {
		return out
	}
	for i := range t.NumField() {
		f := t.Field(i)
		if f.PkgPath != "" { // unexported
			continue
		}
		out = append(out, fieldInfo{
			name:  lowerFirst(f.Name),
			index: i,
			typ:   f.Type,
			codec: d.compile(f.Type),
		})
	}
	return out
}

func lowerFirst(s string) string {
	if s == "" {
		return s
	}
	b := []byte(s)
	if b[0] >= 'A' && b[0] <= 'Z' {
		b[0] += 'a' - 'A'
	}
	return string(b)
}

// DefineAgent declares a config-less agent and returns its state-free
// [AgentDefinition]. Put it in a package-level var in the agent's DEFINITION
// package; other agents import that package to call the agent. The behaviour and
// the private state are attached separately with [Implement] — typically from a
// different (implementation) package, so agents can call one another without a Go
// import cycle.
//
// To attach config, declare the agent with [DefineConfiguredAgent].
func DefineAgent[Id any](spec Spec) *AgentDefinition[Id, NoConfig] {
	return defineAgentInto[Id, NoConfig](defs, spec)
}

// DefineConfiguredAgent declares an agent with config type Cfg and returns its
// state-free [AgentDefinition]. Cfg rides on the definition, so a method reads
// config with [Config] and the constructor reads it with [InitContext.Config].
// Attach the behaviour with [ImplementConfigured] (whose init receives an
// *[InitContext]); an agent whose constructor does not itself need config can
// still use the plain [Implement].
//
//	type ShopConfig struct{ Greeting string }
//	var Shop = golem.DefineConfiguredAgent[ShopId, ShopConfig](golem.Spec{Name: "Shop"})
func DefineConfiguredAgent[Id any, Cfg any](spec Spec) *AgentDefinition[Id, Cfg] {
	return defineAgentInto[Id, Cfg](defs, spec)
}

// defineAgentInto registers the agent's DEFINITION shell (name, mode, mount,
// snapshot policy, id fields, and config surface) on d and returns the handle.
// The constructor and the methods are attached later by [Implement]/[Bound].
// Tests call it with their own definitions for full isolation. (It must stay a
// generic function — Go forbids generic methods.)
func defineAgentInto[Id any, Cfg any](d *definitions, spec Spec) *AgentDefinition[Id, Cfg] {
	idType := reflect.TypeFor[Id]()
	if spec.Name == "" {
		d.recordErr("", "", "DefineAgent requires a non-empty Spec.Name (Id type %s)", idType)
		return &AgentDefinition[Id, Cfg]{name: spec.Name}
	}
	if _, dup := d.agents[spec.Name]; dup {
		d.recordErr(spec.Name, "", "agent type already defined")
		return &AgentDefinition[Id, Cfg]{name: spec.Name}
	}
	if idType.Kind() != reflect.Struct {
		// Record but still register (with no id fields) so a later Implement attaches
		// rather than cascading into "unknown agent" errors.
		d.recordErr(spec.Name, "", "Id must be a struct, got %s", idType)
	}
	e := &agentEntry{
		name:     spec.Name,
		desc:     spec.Description,
		mode:     spec.Mode.toWit(),
		mount:    spec.HTTP,
		snapshot: spec.Snapshot,
		idType:   idType,
		idFields: d.structFields(idType),
		methods:  map[string]*methodEntry{},
		// newState and the methods are attached by Implement; newState stays nil
		// until then (an agent defined but never implemented fails at initialize).
	}
	d.agents[spec.Name] = e
	d.order = append(d.order, spec.Name)
	// Attach Cfg's config surface. NoConfig (the DefineAgent path) has no fields,
	// so this records nothing. Done after registration so it lands on the live entry.
	flattenConfigStruct(d, e, spec.Name, reflect.TypeFor[Cfg]())
	// The Id type identifies the target agent for typed calls (Get), so two agents
	// cannot share one — the second would silently shadow the first.
	if existing, ok := d.idToAgent[idType]; ok && existing != spec.Name {
		d.recordErr(spec.Name, "", "Id type %s is already used by agent %q; each agent needs a distinct Id type", idType, existing)
	} else {
		d.idToAgent[idType] = spec.Name
	}
	return &AgentDefinition[Id, Cfg]{name: spec.Name}
}

// DefineMethod declares a typed method descriptor. The type parameters are
// explicit because none can be inferred from the arguments:
//
//	var Add = golem.DefineMethod[CounterId, AddIn, int64]("add")
func DefineMethod[Id any, In any, Out any](name string, opts ...MethodOpt) MethodDef[Id, In, Out] {
	var o methodOpts
	for _, f := range opts {
		f(&o)
	}
	// descCount is carried on the descriptor and validated at Implement time,
	// where the target definitions is known — DefineMethod itself is instance
	// independent (it just returns a descriptor).
	return MethodDef[Id, In, Out]{
		name: name, desc: o.desc, descCount: o.descCount, endpoints: o.endpoints,
		readOnly: o.readOnly, readOnlyCount: o.readOnlyCount, cacheCount: o.cacheCount,
	}
}

// Implement attaches an agent's constructor (init, which builds the private state
// from the constructor parameters) and returns a state-bound implementation
// handle. Put it in the agent's IMPLEMENTATION package, then register each method
// on the handle with [Handle]. The state type S is introduced here and stays
// private to this package — callers of the agent never see it.
//
//	type state struct{ count int64 }
//	var counter = golem.Implement(counteragent.Agent, func(counteragent.CounterId) *state { return &state{} })
//	func init() {
//	    golem.Handle(counter, counteragent.Add, func(ctx *golem.Context[state], in counteragent.AddIn) int64 {
//	        ctx.State.count += in.By
//	        return ctx.State.count
//	    })
//	}
//
// For an agent whose constructor reads config, use [ImplementConfigured].
func Implement[Id any, S any, Cfg any](
	def *AgentDefinition[Id, Cfg],
	init func(Id) *S,
) *AgentImpl[Id, S, Cfg] {
	return implementInto[Id, S, Cfg](defs, def, simpleNewState[Id, S](init), init == nil)
}

// ImplementConfigured is [Implement] for an agent whose constructor reads config:
// init receives an *[InitContext] carrying the id ([InitContext.ID]) and the
// config ([InitContext.Config]).
func ImplementConfigured[Id any, S any, Cfg any](
	def *AgentDefinition[Id, Cfg],
	init func(*InitContext[Id, S, Cfg]) *S,
) *AgentImpl[Id, S, Cfg] {
	return implementInto[Id, S, Cfg](defs, def, configuredNewState[Id, S, Cfg](init), init == nil)
}

// Handle registers one method handler on the implementation handle returned by
// [Implement] / [ImplementConfigured]. S, In and Out are inferred from the
// handler, and Id must match the descriptor's agent, so binding a descriptor to
// the wrong agent, or a handler with the wrong signature or state type, is a
// compile error. Compose it with the method-expression adapters ([Bind] etc.) to
// author handlers as ordinary Go methods. Call it once per method — typically
// inside func init(). The handler is wrapped once, here, into a uniform
// dispatcher; dispatch itself never uses reflection to call it.
//
// A handler returns only its output value. There is no error return: a failed
// invocation is signalled by panicking (the SDK recovers it into a non-retriable
// agent-error surfaced to the caller — the worker survives). Reserve panic for
// genuine failures; model expected, typed outcomes as a [Result] in the output.
// Use [Must] to turn an inner (value, error) call into a panic-on-error.
func Handle[Id any, S any, Cfg any, In any, Out any](
	impl *AgentImpl[Id, S, Cfg],
	m MethodDef[Id, In, Out],
	h func(*Context[S], In) Out,
) Registered {
	if impl != nil && impl.e != nil {
		bindMethodInto[Id, S, In, Out](impl.d, impl.e, m, h)
	}
	return Registered{}
}

// Registered is the result of [Handle]; it carries no data. It lets a Handle call
// sit in a package-level `var _ = golem.Handle(…)` as an alternative to calling it
// inside func init().
type Registered struct{}

func simpleNewState[Id any, S any](init func(Id) *S) func(reflect.Value, string) any {
	if init == nil {
		return nil
	}
	return func(idVal reflect.Value, _ string) any { return init(idVal.Interface().(Id)) }
}

func configuredNewState[Id any, S any, Cfg any](init func(*InitContext[Id, S, Cfg]) *S) func(reflect.Value, string) any {
	if init == nil {
		return nil
	}
	// No host call here: the constructor reads config lazily via ctx.Config(),
	// keeping get-config-value out of this always-linked path.
	return func(idVal reflect.Value, agentID string) any {
		return init(&InitContext[Id, S, Cfg]{id: idVal.Interface().(Id), agentID: agentID})
	}
}

// implementInto registers the agent's constructor on its already-declared entry
// and returns the state-bound handle that [Handle] adds methods to. Tests call it
// with their own definitions for isolation.
func implementInto[Id any, S any, Cfg any](
	d *definitions,
	def *AgentDefinition[Id, Cfg],
	newState func(reflect.Value, string) any,
	initNil bool,
) *AgentImpl[Id, S, Cfg] {
	e := d.agents[def.name]
	if e == nil {
		d.recordErr(def.name, "", "Implement: unknown agent %q (was DefineAgent called?)", def.name)
		return &AgentImpl[Id, S, Cfg]{d: d}
	}
	if initNil {
		// Recorded, not fatal: init is only called from a successful initialize,
		// gated on this agent having no definition errors.
		d.recordErr(def.name, "", "Implement requires a non-nil init function")
	}
	if e.newState != nil {
		d.recordErr(def.name, "", "agent already implemented")
		return &AgentImpl[Id, S, Cfg]{d: d, e: e}
	}
	e.newState = newState
	return &AgentImpl[Id, S, Cfg]{d: d, e: e}
}

// bindMethodInto registers one method handler on an agent entry: it validates the
// descriptor, compiles the in/out codecs once (not per invocation), and installs
// the erased dispatcher (dispatch never uses reflection to reach the handler).
func bindMethodInto[Id any, S any, In any, Out any](
	d *definitions,
	e *agentEntry,
	m MethodDef[Id, In, Out],
	h func(*Context[S], In) Out,
) {
	if m.name == "" {
		d.recordErr(e.name, "", "DefineMethod requires a non-empty method name")
		return
	}
	if h == nil {
		d.recordErr(e.name, m.name, "Handle requires a non-nil handler")
		return
	}
	if m.descCount > 1 {
		d.recordErr(e.name, m.name, "method %q: Desc set %d times (a method has one description)", m.name, m.descCount)
	}
	if m.readOnlyCount > 1 {
		d.recordErr(e.name, m.name, "method %q: ReadOnly set %d times (a method is read-only once)", m.name, m.readOnlyCount)
	}
	if m.cacheCount > 1 {
		d.recordErr(e.name, m.name, "method %q: ReadOnly accepts at most one cache policy, got %d", m.name, m.cacheCount)
	}
	if m.readOnly != nil {
		if m.readOnly.policy.kind == cacheTTL && m.readOnly.policy.ttl <= 0 {
			d.recordErr(e.name, m.name, "method %q: CacheFor requires a positive ttl, got %v (use NoCache to disable caching)", m.name, m.readOnly.policy.ttl)
		}
		if e.mode == common.AgentModeEphemeral {
			d.recordErr(e.name, m.name, "method %q: ReadOnly is only valid on a Durable agent (an ephemeral agent has no shared state to cache)", m.name)
		}
	}
	if _, dup := e.methods[m.name]; dup {
		d.recordErr(e.name, m.name, "method already implemented")
		return
	}

	// Codecs are compiled once, here at registration — not per invocation.
	inType := reflect.TypeFor[In]()
	outType := reflect.TypeFor[Out]()
	me := &methodEntry{name: m.name, desc: m.desc, inFields: d.structFields(inType), endpoints: m.endpoints, readOnly: m.readOnly}
	if outType != reflect.TypeFor[Unit]() {
		me.outCodec = d.compile(outType)
	}

	me.invoke = func(state any, agentID string, tree types.SchemaValueTree) (out *types.SchemaValueTree, err error) {
		// Panic hardening: a panic becomes an agent-error instead of killing the
		// component. stage attributes it, so an SDK bug is not reported as if it
		// were the agent's fault.
		stage := stageDecode
		defer func() {
			if r := recover(); r != nil {
				out, err = nil, &PanicError{Method: me.name, Stage: stage, Value: r}
			}
		}()

		inVal := reflect.New(inType).Elem()
		if derr := decodeParams(tree, me.inFields, inVal); derr != nil {
			return nil, &decodeError{derr.Error()}
		}

		stage = stageHandler
		ctx := &Context[S]{State: state.(*S), agentID: agentID}
		result := h(ctx, inVal.Interface().(In))
		if me.outCodec == nil {
			return nil, nil
		}

		stage = stageEncode
		// &result, not result: reflect.ValueOf unwraps an interface to the
		// concrete type it holds, which would lose the declared type of a
		// variant-typed output.
		encoded := encodeWith(me.outCodec, reflect.ValueOf(&result).Elem())
		return &encoded, nil
	}

	e.methods[m.name] = me
	e.order = append(e.order, m.name)
}

// ---------------------------------------------------------------------------
// Method-expression binding
//
// A Go method expression ((*S).Method) is a typed func value whose first
// argument is the receiver, so these adapters allow authoring agents as ordinary
// Go methods with full compile-time checking and no reflection:
//
//	func (s *CartState) AddItem(in AddItemIn) int64 { ... }
//	golem.Implement(Cart, CartAdd, golem.Bind((*CartState).AddItem))
//
// Methods signal failure by panicking, like any handler. Go has no overloading,
// so there is one adapter per method shape: `0` = no input, `Unit` = no output.
// ---------------------------------------------------------------------------

// Bind adapts func(*S, In) Out — the canonical shape.
func Bind[S, In, Out any](m func(*S, In) Out) func(*Context[S], In) Out {
	return func(ctx *Context[S], in In) Out { return m(ctx.State, in) }
}

// Bind0 adapts func(*S) Out — no input.
func Bind0[S, Out any](m func(*S) Out) func(*Context[S], Unit) Out {
	return func(ctx *Context[S], _ Unit) Out { return m(ctx.State) }
}

// BindUnit adapts func(*S, In) — no output.
func BindUnit[S, In any](m func(*S, In)) func(*Context[S], In) Unit {
	return func(ctx *Context[S], in In) Unit { m(ctx.State, in); return Unit{} }
}

// Bind0Unit adapts func(*S) — neither input nor output.
func Bind0Unit[S any](m func(*S)) func(*Context[S], Unit) Unit {
	return func(ctx *Context[S], _ Unit) Unit { m(ctx.State); return Unit{} }
}
