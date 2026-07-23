// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"fmt"
	"reflect"

	common "github.com/golemcloud/golem/sdks/go/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/internal/wit/golem_core_types"
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
	name     string
	desc     string
	inFields []fieldInfo
	outCodec *codec // nil => unit output
	// invoke is the erased dispatcher produced by Implement. Calling it is a
	// direct func-value call: no reflection is used to reach the handler.
	invoke func(state any, agentID string, in types.SchemaValueTree) (out *types.SchemaValueTree, err error)
}

type agentEntry struct {
	name     string
	desc     string
	mode     common.AgentMode
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

var (
	registry      = map[string]*agentEntry{}
	registryOrder []string
	// idTypeToAgent lets ClientFor resolve a target agent from its Id type.
	idTypeToAgent = map[reflect.Type]string{}
	// active is the instance this worker was initialized as, or nil before
	// initialize() has run.
	active *instance
)

// structFields returns the exported fields of a struct type in declaration
// order. Non-structs (e.g. Unit) yield no fields.
func structFields(t reflect.Type) []fieldInfo {
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
			codec: compile(f.Type),
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
func DefineAgent[Id any, S any](spec Spec, init func(Id) *S) *Agent[Id, S] {
	if spec.Name == "" {
		panic("golem: DefineAgent requires a non-empty Spec.Name")
	}
	if _, dup := registry[spec.Name]; dup {
		panic("golem: agent type already defined: " + spec.Name)
	}
	idType := reflect.TypeFor[Id]()
	if idType.Kind() != reflect.Struct {
		panic(fmt.Sprintf("golem: agent %s: Id must be a struct, got %s", spec.Name, idType))
	}
	e := &agentEntry{
		name:     spec.Name,
		desc:     spec.Description,
		mode:     spec.Mode.toWit(),
		idType:   idType,
		idFields: structFields(idType),
		methods:  map[string]*methodEntry{},
		newState: func(idVal reflect.Value) any { return init(idVal.Interface().(Id)) },
	}
	registry[spec.Name] = e
	registryOrder = append(registryOrder, spec.Name)
	idTypeToAgent[idType] = spec.Name
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
	return MethodDef[Id, In, Out]{name: name, desc: o.desc}
}

// Implement binds a handler to a method descriptor. S, In and Out are inferred
// from the handler, and Id must match the agent — so binding a descriptor to the
// wrong agent, or a handler with the wrong signature, is a compile error.
//
// The handler is wrapped once, here, into a uniform dispatcher; dispatch itself
// never uses reflection to call user code.
func Implement[Id any, S any, In any, Out any](
	a *Agent[Id, S],
	m MethodDef[Id, In, Out],
	h func(*Context[S], In) (Out, error),
) {
	e := registry[a.name]
	if e == nil {
		panic("golem: Implement: unknown agent " + a.name)
	}
	if _, dup := e.methods[m.name]; dup {
		panic("golem: " + a.name + ": method already implemented: " + m.name)
	}

	// Codecs are compiled once, here at registration — not per invocation.
	inType := reflect.TypeFor[In]()
	outType := reflect.TypeFor[Out]()
	me := &methodEntry{name: m.name, desc: m.desc, inFields: structFields(inType)}
	if outType != reflect.TypeFor[Unit]() {
		me.outCodec = compile(outType)
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
		result, herr := h(ctx, inVal.Interface().(In))
		if herr != nil {
			return nil, herr
		}
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
//	func (s *CartState) AddItem(in AddItemIn) (int64, error) { ... }
//	golem.Implement(Cart, CartAdd, golem.Bind((*CartState).AddItem))
//
// Go has no overloading, so there is one adapter per method shape: `0` = no
// input, `NoErr` = cannot fail, `Unit` = no output.
// ---------------------------------------------------------------------------

// Bind adapts func(*S, In) (Out, error) — the canonical shape.
func Bind[S, In, Out any](m func(*S, In) (Out, error)) func(*Context[S], In) (Out, error) {
	return func(ctx *Context[S], in In) (Out, error) { return m(ctx.State, in) }
}

// BindNoErr adapts func(*S, In) Out.
func BindNoErr[S, In, Out any](m func(*S, In) Out) func(*Context[S], In) (Out, error) {
	return func(ctx *Context[S], in In) (Out, error) { return m(ctx.State, in), nil }
}

// Bind0 adapts func(*S) (Out, error) — no input.
func Bind0[S, Out any](m func(*S) (Out, error)) func(*Context[S], Unit) (Out, error) {
	return func(ctx *Context[S], _ Unit) (Out, error) { return m(ctx.State) }
}

// Bind0NoErr adapts func(*S) Out.
func Bind0NoErr[S, Out any](m func(*S) Out) func(*Context[S], Unit) (Out, error) {
	return func(ctx *Context[S], _ Unit) (Out, error) { return m(ctx.State), nil }
}

// BindUnit adapts func(*S, In) — no output.
func BindUnit[S, In any](m func(*S, In)) func(*Context[S], In) (Unit, error) {
	return func(ctx *Context[S], in In) (Unit, error) { m(ctx.State, in); return Unit{}, nil }
}

// Bind0Unit adapts func(*S) — neither input nor output.
func Bind0Unit[S any](m func(*S)) func(*Context[S], Unit) (Unit, error) {
	return func(ctx *Context[S], _ Unit) (Unit, error) { m(ctx.State); return Unit{}, nil }
}
