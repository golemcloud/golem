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
	endpoints []Endpoint // HTTP routes, if any
	outCodec  *codec     // nil => unit output
	// invoke is the erased dispatcher produced by Implement. Calling it is a
	// direct func-value call: no reflection is used to reach the handler.
	invoke func(state any, agentID string, in types.SchemaValueTree) (out *types.SchemaValueTree, err error)
}

type agentEntry struct {
	name     string
	desc     string
	mode     common.AgentMode
	mount    *Mount // HTTP mount, if any
	idType   reflect.Type
	idFields []fieldInfo
	newState func(idVal reflect.Value) any
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

// DefineAgent registers an agent type and returns its typed handle. init builds
// the instance state from the constructor parameters.
//
// Call it from a package-level var so registration happens before the component
// is invoked.
//
// Thin wrapper over the package-global defs — keep all logic in
// [defineAgentInto] so it stays testable against an explicit *definitions. See
// [defs].
func DefineAgent[Id any, S any](spec Spec, init func(Id) *S) *Agent[Id, S] {
	return defineAgentInto[Id, S](defs, spec, init)
}

// defineAgentInto is the instance-scoped implementation. The public DefineAgent
// wraps it against the package-global defs; tests call it with their own
// definitions for full isolation. (It must stay a generic function — Go forbids
// generic methods.)
func defineAgentInto[Id any, S any](d *definitions, spec Spec, init func(Id) *S) *Agent[Id, S] {
	idType := reflect.TypeFor[Id]()
	if spec.Name == "" {
		d.recordErr("", "", "DefineAgent requires a non-empty Spec.Name (Id type %s)", idType)
		return &Agent[Id, S]{name: spec.Name}
	}
	if _, dup := d.agents[spec.Name]; dup {
		d.recordErr(spec.Name, "", "agent type already defined")
		return &Agent[Id, S]{name: spec.Name}
	}
	if idType.Kind() != reflect.Struct {
		// Record but still register (with no id fields) so downstream Implement
		// calls attach rather than cascading into "unknown agent" errors.
		d.recordErr(spec.Name, "", "Id must be a struct, got %s", idType)
	}
	if init == nil {
		// Recorded, not fatal: init is only called from a successful initialize,
		// which is gated on this agent having no definition errors — so the nil is
		// reported at discovery rather than panicking at construction time.
		d.recordErr(spec.Name, "", "DefineAgent requires a non-nil init function")
	}
	e := &agentEntry{
		name:     spec.Name,
		desc:     spec.Description,
		mode:     spec.Mode.toWit(),
		mount:    spec.HTTP,
		idType:   idType,
		idFields: d.structFields(idType),
		methods:  map[string]*methodEntry{},
		newState: func(idVal reflect.Value) any { return init(idVal.Interface().(Id)) },
	}
	d.agents[spec.Name] = e
	d.order = append(d.order, spec.Name)
	// The Id type identifies the target agent for typed calls (ClientFor), so two
	// agents cannot share one — the second would silently shadow the first.
	if existing, ok := d.idToAgent[idType]; ok && existing != spec.Name {
		d.recordErr(spec.Name, "", "Id type %s is already used by agent %q; each agent needs a distinct Id type", idType, existing)
	} else {
		d.idToAgent[idType] = spec.Name
	}
	return &Agent[Id, S]{name: spec.Name}
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
	return MethodDef[Id, In, Out]{name: name, desc: o.desc, descCount: o.descCount, endpoints: o.endpoints}
}

// Implement binds a handler to a method descriptor. S, In and Out are inferred
// from the handler, and Id must match the agent — so binding a descriptor to the
// wrong agent, or a handler with the wrong signature, is a compile error.
//
// The handler is wrapped once, here, into a uniform dispatcher; dispatch itself
// never uses reflection to call user code.
// A handler returns only its output value. There is no error return: a failed
// invocation is signalled by panicking (the SDK recovers it into a non-retriable
// agent-error surfaced to the caller — the worker survives). Reserve panic for
// genuine failures; model expected, typed outcomes as a [Result] in the output.
// Use [Must] to turn an inner (value, error) call into a panic-on-error.
//
// Thin wrapper over the package-global defs — keep all logic in [implementInto]
// so it stays testable against an explicit *definitions. See [defs].
func Implement[Id any, S any, In any, Out any](
	a *Agent[Id, S],
	m MethodDef[Id, In, Out],
	h func(*Context[S], In) Out,
) {
	implementInto[Id, S, In, Out](defs, a, m, h)
}

// implementInto is the instance-scoped implementation behind Implement.
func implementInto[Id any, S any, In any, Out any](
	d *definitions,
	a *Agent[Id, S],
	m MethodDef[Id, In, Out],
	h func(*Context[S], In) Out,
) {
	e := d.agents[a.name]
	if e == nil {
		d.recordErr(a.name, m.name, "Implement: unknown agent %q (was DefineAgent called?)", a.name)
		return
	}
	if m.name == "" {
		d.recordErr(a.name, "", "DefineMethod requires a non-empty method name")
		return
	}
	if h == nil {
		d.recordErr(a.name, m.name, "Implement requires a non-nil handler")
		return
	}
	if m.descCount > 1 {
		d.recordErr(a.name, m.name, "method %q: Desc set %d times (a method has one description)", m.name, m.descCount)
	}
	if _, dup := e.methods[m.name]; dup {
		d.recordErr(a.name, m.name, "method already implemented")
		return
	}

	// Codecs are compiled once, here at registration — not per invocation.
	inType := reflect.TypeFor[In]()
	outType := reflect.TypeFor[Out]()
	me := &methodEntry{name: m.name, desc: m.desc, inFields: d.structFields(inType), endpoints: m.endpoints}
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
