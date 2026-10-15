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

package tool

import (
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"io"
	"reflect"
	"slices"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool middleware.
//
// A middleware wraps the invocations of a tool: it sees a call on its way in,
// hands it to the layer beneath, and sees the outcome on its way back. It is
// declared on the tool it presents, and handles that tool's commands with
// their own argument and result types:
//
//	var Policy = inventory.Tool.Middleware[PolicyParams]("inventory-policy", tool.MiddlewareSpec{Version: "1.0.0"})
//
//	var _ = Policy.Handle(inventory.Adjust, func(ctx *tool.MiddlewareContext[PolicyParams], a inventory.AdjustArgs) (int32, error) {
//	    if a.By > ctx.Parameters().MaxAdjust {
//	        return 0, errors.New("adjustment too large")
//	    }
//	    return Policy.Underlying(ctx, inventory.Adjust).Forward(a)
//	})
//
// A transparent middleware (Middleware) presents and wraps the same tool, and
// a command it does not handle goes straight to the tool. An adapter (Adapter)
// presents one tool over another, so it handles every presented command and
// calls the wrapped tool's commands beneath.
//
// A middleware that applies to any tool, such as an audit log, has no Go types
// for the tools it wraps; it is declared with [DefineUniversalToolMiddleware]
// and works with [TypedValue].
//
// P is the middleware's static configuration, whose schema is published so an
// installation can be checked before it runs. Use [Unit] for none.

// MiddlewareSpec describes a middleware as a whole.
type MiddlewareSpec struct {
	// Version is the middleware's own version; it defaults to 0.0.0.
	Version     string
	Summary     string
	Description string
	Aliases     []string
}

// middlewareEntry is one registered middleware.
type middlewareEntry struct {
	name       string
	spec       MiddlewareSpec
	paramsType reflect.Type
	d          *definitions

	// presented and wrapped are the tools of a typed middleware, the same
	// tool for a transparent one; nil for a universal one.
	presented *toolEntry
	wrapped   *toolEntry
	adapter   bool
	handlers  map[*commandEntry]middlewareHandler

	// universal is the handler of a universal middleware.
	universal func(*middlewareInvocation) (golem.Option[golem.TypedValue], error)
}

type middlewareHandler func(*middlewareInvocation, reflect.Value) (reflect.Value, error)

func (e *middlewareEntry) fail(format string, args ...any) {
	e.d.RecordErr("", "", "tool middleware %s: %s", e.name, fmt.Sprintf(format, args...))
}

func registerMiddleware(r *toolRegistry, d *definitions, e *middlewareEntry) {
	switch {
	case e.name == "":
		d.RecordErr("", "", "a tool middleware requires a name")
	case r.middlewaresByName[e.name] != nil:
		d.RecordErr("", "", "tool middleware already defined: %s", e.name)
	default:
		r.middlewareOrder = append(r.middlewareOrder, e.name)
		r.middlewaresByName[e.name] = e
	}
}

// Middleware is a typed middleware presenting tool T, wrapping tool W and
// configured by P.
type Middleware[T any, P any, W any] struct{ e *middlewareEntry }

// Name returns the middleware's name.
func (m *Middleware[T, P, W]) Name() string { return m.e.name }

// Middleware declares a transparent middleware on this tool: it presents and
// wraps the tool, and a command it does not handle goes straight to the tool.
func (t *Definition[T]) Middleware[P any](name string, spec MiddlewareSpec) *Middleware[T, P, T] {
	return &Middleware[T, P, T]{e: newTypedMiddleware[P](t.entry, t.entry, name, spec, false)}
}

// Adapter declares a middleware presenting this tool over wraps: it handles
// every command of this tool, calling the commands of wraps beneath.
func (t *Definition[T]) Adapter[P any, W any](name string, wraps *Definition[W], spec MiddlewareSpec) *Middleware[T, P, W] {
	return &Middleware[T, P, W]{e: newTypedMiddleware[P](t.entry, wraps.entry, name, spec, true)}
}

func newTypedMiddleware[P any](presented, wrapped *toolEntry, name string, spec MiddlewareSpec, adapter bool) *middlewareEntry {
	e := &middlewareEntry{
		name: name, spec: spec, paramsType: reflect.TypeFor[P](), d: presented.d,
		presented: presented, wrapped: wrapped, adapter: adapter,
		handlers: map[*commandEntry]middlewareHandler{},
	}
	registerMiddleware(presented.r, presented.d, e)
	return e
}

func (e *middlewareEntry) setHandler(ce *commandEntry, h middlewareHandler) golem.Registered {
	switch {
	case ce.node.entry != e.presented:
		e.fail("handles %s of tool %s, which it does not present", ce.label(), ce.node.entry.name)
	case e.handlers[ce] != nil:
		e.fail("handles command %s twice", ce.label())
	default:
		e.handlers[ce] = h
	}
	return golem.Registered{}
}

// Handle intercepts a command of the presented tool. The handler returns the
// command's result as the tool would; a declared error is returned with the
// case's New, and a failure of the layer beneath is passed on unchanged by
// returning it.
func (m *Middleware[T, P, W]) Handle[A any, O any](
	cmd *Command[T, A, O], h func(*MiddlewareContext[P], A) (O, error),
) golem.Registered {
	return m.e.setHandler(cmd.ce, func(inv *middlewareInvocation, args reflect.Value) (reflect.Value, error) {
		ctx, err := newMiddlewareContext[P](inv)
		if err != nil {
			return reflect.Value{}, err
		}
		out, err := h(ctx, args.Interface().(A))
		return reflect.ValueOf(&out).Elem(), err
	})
}

// HandleOutput intercepts a command of the presented tool that has outputs,
// which the handler writes through [MiddlewareOutputContext.Stdout] and
// [MiddlewareOutputContext.Stderr].
func (m *Middleware[T, P, W]) HandleOutput[A any, O any](
	cmd *OutputCommand[T, A, O], h func(*MiddlewareOutputContext[P], A) (O, error),
) golem.Registered {
	return m.e.setHandler(cmd.ce, func(inv *middlewareInvocation, args reflect.Value) (reflect.Value, error) {
		ctx, err := newMiddlewareContext[P](inv)
		if err != nil {
			return reflect.Value{}, err
		}
		out, err := h(&MiddlewareOutputContext[P]{MiddlewareContext: ctx}, args.Interface().(A))
		return reflect.ValueOf(&out).Elem(), err
	})
}

// Underlying reaches a command of the wrapped tool for this invocation.
func (m *Middleware[T, P, W]) Underlying[A any, O any](ctx MiddlewareCall, cmd *Command[W, A, O]) *Underlying[A, O] {
	return &Underlying[A, O]{inv: ctx.middlewareInvocation(), ce: cmd.ce, m: m.e}
}

// UnderlyingOutput reaches a command of the wrapped tool that has outputs.
func (m *Middleware[T, P, W]) UnderlyingOutput[A any, O any](ctx MiddlewareCall, cmd *OutputCommand[W, A, O]) *UnderlyingOutput[A, O] {
	return &UnderlyingOutput[A, O]{inv: ctx.middlewareInvocation(), ce: cmd.ce, m: m.e}
}

// MiddlewareCall is the context of a running middleware handler, which is
// what reaching the layer beneath needs.
type MiddlewareCall interface {
	middlewareInvocation() *middlewareInvocation
}

// MiddlewareContext is the per-invocation context of a typed middleware.
type MiddlewareContext[P any] struct {
	inv    *middlewareInvocation
	params P
}

func newMiddlewareContext[P any](inv *middlewareInvocation) (*MiddlewareContext[P], error) {
	params, err := golem.DecodeTypedValue[P](inv.parameters)
	if err != nil {
		return nil, fmt.Errorf("middleware %s: parameters: %w", inv.entry.name, err)
	}
	return &MiddlewareContext[P]{inv: inv, params: params}, nil
}

func (c *MiddlewareContext[P]) middlewareInvocation() *middlewareInvocation { return c.inv }

// Parameters returns the middleware's configuration for this installation.
func (c *MiddlewareContext[P]) Parameters() P { return c.params }

// Principal returns who invoked the tool.
func (c *MiddlewareContext[P]) Principal() golem.Principal { return c.inv.principal }

// Name returns the middleware's own name.
func (c *MiddlewareContext[P]) Name() string { return c.inv.entry.name }

// ToolName returns the name the tool was invoked by.
func (c *MiddlewareContext[P]) ToolName() string { return c.inv.toolName }

// CommandPath returns the command being invoked, from the tool's root.
func (c *MiddlewareContext[P]) CommandPath() []string { return slices.Clone(c.inv.commandPath) }

// MiddlewareOutputContext is the context of a handler for a command that
// has outputs.
type MiddlewareOutputContext[P any] struct {
	*MiddlewareContext[P]
}

// Stdout returns the middleware's standard output. It is finished when the
// handler succeeds and failed when it returns an error or panics.
func (c *MiddlewareOutputContext[P]) Stdout() *Output { return c.inv.stdout }

// Stderr returns the middleware's standard error, finished and failed like
// Stdout.
func (c *MiddlewareOutputContext[P]) Stderr() *Output { return c.inv.stderr }

// Underlying is a command of the wrapped tool, reached from a middleware
// handler.
type Underlying[A any, O any] struct {
	inv *middlewareInvocation
	ce  *commandEntry
	m   *middlewareEntry
}

// Call runs the command beneath, starting from its declared defaults.
func (u *Underlying[A, O]) Call(fill func(*A)) (O, error) {
	return finishUnderlying[O](u.ce, u.inv, u.m, fillArgs(fill))
}

// Forward runs the command beneath with complete arguments, unchanged.
func (u *Underlying[A, O]) Forward(a A) (O, error) {
	return finishUnderlying[O](u.ce, u.inv, u.m, forwardArgs(a))
}

// UnderlyingOutput is a command of the wrapped tool that has outputs,
// reached from a middleware handler. An output of the running call that the
// handler does not take passes through to the middleware's own.
type UnderlyingOutput[A any, O any] struct {
	inv *middlewareInvocation
	ce  *commandEntry
	m   *middlewareEntry
}

// Call starts the command beneath, starting from its declared defaults.
func (u *UnderlyingOutput[A, O]) Call(fill func(*A)) (*Invocation[O], error) {
	return startUnderlying[O](u.ce, u.inv, u.m, fillArgs(fill))
}

// Start starts the command beneath with complete arguments, unchanged.
func (u *UnderlyingOutput[A, O]) Start(a A) (*Invocation[O], error) {
	return startUnderlying[O](u.ce, u.inv, u.m, forwardArgs(a))
}

// Forward runs the command beneath, passing its outputs through to the
// middleware's own, and returns its result.
func (u *UnderlyingOutput[A, O]) Forward(a A) (O, error) {
	return finishUnderlying[O](u.ce, u.inv, u.m, forwardArgs(a))
}

func forwardArgs[A any](a A) func(reflect.Value) {
	return func(v reflect.Value) { v.Set(reflect.ValueOf(&a).Elem()) }
}

func startUnderlying[O any](ce *commandEntry, inv *middlewareInvocation, m *middlewareEntry, fill func(reflect.Value)) (*Invocation[O], error) {
	if ce.node.entry != m.wrapped {
		return nil, fmt.Errorf("golem: middleware %s calls %s of tool %s, which it does not wrap",
			m.name, ce.label(), ce.node.entry.name)
	}
	input, stdin, err := ce.prepare(inv.toolName, fill)
	if err != nil {
		return nil, err
	}
	call, err := inv.under.start(ce.node.path, input, stdin)
	if err != nil {
		return nil, err
	}
	started := commandInvocation[O](ce, inv.toolName, call)
	started.passThrough = inv
	return started, nil
}

func finishUnderlying[O any](ce *commandEntry, inv *middlewareInvocation, m *middlewareEntry, fill func(reflect.Value)) (O, error) {
	var zero O
	started, err := startUnderlying[O](ce, inv, m, fill)
	if err != nil {
		return zero, err
	}
	return started.Wait()
}

// UniversalMiddleware is a middleware for any tool, configured by P.
type UniversalMiddleware[P any] struct{ e *middlewareEntry }

// Name returns the middleware's name.
func (m *UniversalMiddleware[P]) Name() string { return m.e.name }

// DefineUniversalToolMiddleware declares a middleware that applies to any tool.
// It has no Go types for the tools it wraps, so the arguments and the result
// travel as [TypedValue]. Call it from a package-level var.
func DefineUniversalToolMiddleware[P any](name string, spec MiddlewareSpec) *UniversalMiddleware[P] {
	return defineUniversalToolMiddlewareInto[P](toolDefs, defs, name, spec)
}

func defineUniversalToolMiddlewareInto[P any](r *toolRegistry, d *definitions, name string, spec MiddlewareSpec) *UniversalMiddleware[P] {
	e := &middlewareEntry{name: name, spec: spec, paramsType: reflect.TypeFor[P](), d: d}
	registerMiddleware(r, d, e)
	return &UniversalMiddleware[P]{e: e}
}

// Handle binds the middleware's implementation. The handler returns the
// result to hand back, none for a command without one; returning a failure of
// the layer beneath passes it on unchanged.
func (m *UniversalMiddleware[P]) Handle(h func(*UniversalMiddlewareContext[P]) (golem.Option[golem.TypedValue], error)) golem.Registered {
	if m.e.universal != nil {
		m.e.fail("already has a handler")
		return golem.Registered{}
	}
	m.e.universal = func(inv *middlewareInvocation) (golem.Option[golem.TypedValue], error) {
		params, err := golem.DecodeTypedValue[P](inv.parameters)
		if err != nil {
			return golem.None[golem.TypedValue](), fmt.Errorf("middleware %s: parameters: %w", inv.entry.name, err)
		}
		return h(&UniversalMiddlewareContext[P]{inv: inv, params: params})
	}
	return golem.Registered{}
}

// UniversalMiddlewareContext is the per-invocation context of a universal
// middleware.
type UniversalMiddlewareContext[P any] struct {
	inv    *middlewareInvocation
	params P
}

// Parameters returns the middleware's configuration for this installation.
func (c *UniversalMiddlewareContext[P]) Parameters() P { return c.params }

// Principal returns who invoked the tool.
func (c *UniversalMiddlewareContext[P]) Principal() golem.Principal { return c.inv.principal }

// Name returns the middleware's own name.
func (c *UniversalMiddlewareContext[P]) Name() string { return c.inv.entry.name }

// ToolName returns the name the tool was invoked by.
func (c *UniversalMiddlewareContext[P]) ToolName() string { return c.inv.toolName }

// Metadata returns the wrapped tool's published metadata, which is how a
// universal middleware learns the shape of a tool it was not written for. Read
// it with reflection.ToolOf.
func (c *UniversalMiddlewareContext[P]) Metadata() Metadata {
	return Metadata{name: c.inv.toolName, wit: c.inv.tool}
}

// Metadata is a tool's published metadata as a middleware receives it. It is
// read through reflection.ToolOf, which turns it into a discovered tool.
type Metadata struct {
	name string
	wit  toolCommon.Tool
}

// CommandPath returns the command being invoked, from the tool's root.
func (c *UniversalMiddlewareContext[P]) CommandPath() []string {
	return slices.Clone(c.inv.commandPath)
}

// Input returns the call's arguments, the command's canonical input record.
func (c *UniversalMiddlewareContext[P]) Input() golem.TypedValue {
	return typedValue(c.inv.input)
}

// Stdout returns the middleware's standard output. It is finished when the
// handler succeeds and failed when it returns an error or panics.
func (c *UniversalMiddlewareContext[P]) Stdout() *Output { return c.inv.stdout }

// Stderr returns the middleware's standard error, finished and failed like
// Stdout.
func (c *UniversalMiddlewareContext[P]) Stderr() *Output { return c.inv.stderr }

// Start hands the call to the layer beneath with the given input and the
// original standard input, and returns the running invocation. An output the
// handler does not take passes through to the middleware's own.
func (c *UniversalMiddlewareContext[P]) Start(input golem.TypedValue) (*Invocation[golem.Option[golem.TypedValue]], error) {
	return c.inv.startRaw(witOf(input))
}

// Next hands the call to the layer beneath, passing its outputs through to
// the middleware's own, and returns its result.
func (c *UniversalMiddlewareContext[P]) Next(input golem.TypedValue) (golem.Option[golem.TypedValue], error) {
	inv, err := c.Start(input)
	if err != nil {
		return golem.None[golem.TypedValue](), err
	}
	return inv.Wait()
}

// middlewareInvocation is the state of one middleware invocation.
type middlewareInvocation struct {
	entry       *middlewareEntry
	toolName    string
	tool        toolCommon.Tool
	parameters  golem.TypedValue
	commandPath []string
	input       types.TypedSchemaValue
	stdin       *byteReader
	stdout      *Output
	stderr      *Output
	principal   golem.Principal
	under       underlyingLayer
}

func (inv *middlewareInvocation) outputs() []*Output {
	return []*Output{inv.stdout, inv.stderr}
}

// startRaw starts the invoked command beneath with the given input and the
// original standard input.
func (inv *middlewareInvocation) startRaw(input types.TypedSchemaValue) (*Invocation[golem.Option[golem.TypedValue]], error) {
	var stdin io.Reader
	if inv.stdin.present() {
		stdin = inv.stdin
	}
	call, err := inv.under.start(inv.commandPath, input, stdin)
	if err != nil {
		return nil, err
	}
	tool, path := inv.toolName, inv.commandPath
	started := newInvocation(call, func(call toolCall) (golem.Option[golem.TypedValue], error) {
		res, rpcErr := call.wait()
		if rpcErr != nil {
			return golem.None[golem.TypedValue](), toolCallErrorFromWit(tool, path, *rpcErr)
		}
		if res.IsNone() {
			return golem.None[golem.TypedValue](), nil
		}
		return golem.Some(typedValue(res.Some())), nil
	})
	started.passThrough = inv
	return started, nil
}

// undeclare makes the middleware's outputs that the presented command does
// not declare fail a write, as a tool's own would.
func (inv *middlewareInvocation) undeclare(ce *commandEntry) {
	st := ce.spec.settings
	if st.stdout == nil {
		inv.stdout.undeclared = fmt.Sprintf("golem: command %s declares no stdout", ce.label())
	}
	if st.stderr == nil {
		inv.stderr.undeclared = fmt.Sprintf("golem: command %s declares no stderr", ce.label())
	}
}

// underlyingLayer is the layer beneath a middleware. It is a struct of
// functions so the dispatcher can be tested without a host; the wasm build
// binds it to the generated resource (see toolmiddleware_wasm.go).
type underlyingLayer struct {
	start func(path []string, input types.TypedSchemaValue, stdin io.Reader) (toolCall, error)
}

// absentUnderlyingLayer stands in when there is no layer beneath.
var absentUnderlyingLayer = underlyingLayer{
	start: func([]string, types.TypedSchemaValue, io.Reader) (toolCall, error) {
		return toolCall{}, fmt.Errorf("golem: this middleware was invoked without a layer beneath it")
	},
}

// buildToolMiddleware derives the metadata the host discovers for one
// middleware.
func (d *definitions) buildToolMiddleware(e *middlewareEntry) (toolCommon.ToolMiddleware, bool) {
	ok := true
	switch {
	case e.presented == nil && e.universal == nil:
		e.fail("has no handler; call Handle on it")
		ok = false
	case e.presented != nil && len(e.handlers) == 0:
		e.fail("handles no command; call Handle on it")
		ok = false
	}

	g := engine.GraphBuilder{E: d.Engine}
	paramsRoot := g.Node(d.Compile(e.paramsType))
	paramsGraph := g.Build()
	paramsGraph.Root = paramsRoot
	for typ, why := range g.Invalids {
		e.fail("takes parameters of %s, which cannot be represented: %s", typ, why)
		ok = false
	}

	scope := toolCommon.MakeToolMiddlewareScopeUniversal()
	if e.presented != nil {
		if e.adapter {
			for _, ce := range e.presented.commands() {
				if e.handlers[ce] == nil {
					e.fail("adapts %s but does not handle its command %s", e.presented.name, ce.label())
					ok = false
				}
			}
		}
		presented, presentedOK := d.buildTool(e.presented)
		wrapped, wrappedOK := d.buildTool(e.wrapped)
		if presentedOK && wrappedOK {
			scope = toolCommon.MakeToolMiddlewareScopeMonomorphic(toolCommon.MonomorphicScope{
				Presented: presented,
				Expected:  witTypes.Some(wrapped),
			})
		} else {
			e.fail("presents or wraps a tool that is not well-defined")
			ok = false
		}
	}

	return toolCommon.ToolMiddleware{
		Name:            e.name,
		Version:         versionOrDefault(e.spec.Version),
		Aliases:         slices.Clone(e.spec.Aliases),
		Doc:             toolDoc{summary: e.spec.Summary, description: e.spec.Description}.toWit(),
		Scope:           scope,
		ParameterSchema: paramsGraph,
	}, ok
}

// commands lists the tool's command bodies in tree order.
func (e *toolEntry) commands() []*commandEntry {
	var out []*commandEntry
	var walk func(n *toolNode)
	walk = func(n *toolNode) {
		if n.body != nil {
			out = append(out, n.body)
		}
		for _, c := range n.children {
			walk(c)
		}
	}
	walk(e.root)
	return out
}

// invokeMiddleware runs one middleware layer.
func (d *definitions) invokeMiddleware(name string, inv *middlewareInvocation) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	fail := witTypes.Err[toolCommon.InvocationResult, types.ToolError]
	defer inv.stdin.close()
	e, known := toolDefs.getMiddleware(name)
	if !known {
		return fail(types.MakeToolErrorInvalidToolName(name))
	}
	return d.runMiddleware(e, inv)
}

func (d *definitions) runMiddleware(e *middlewareEntry, inv *middlewareInvocation) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	fail := witTypes.Err[toolCommon.InvocationResult, types.ToolError]
	inv.entry = e
	label := "middleware " + e.name

	if e.presented == nil {
		if e.universal == nil {
			return fail(toolDefinitionError(d))
		}
		out, err := runWithOutputs(label, inv.outputs(), func() (reflect.Value, error) {
			res, err := e.universal(inv)
			return reflect.ValueOf(res), err
		})
		if err != nil {
			return fail(passThroughToolError(err))
		}
		result := witTypes.None[types.TypedSchemaValue]()
		if v, has := out.Interface().(golem.Option[golem.TypedValue]).Get(); has {
			result = witTypes.Some(witOf(v))
		}
		return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
			Result: result, Stdout: witTypes.None[*witTypes.StreamReader[uint8]](), Stderr: witTypes.None[*witTypes.StreamReader[uint8]](),
		})
	}

	n := e.presented.root.find(inv.commandPath)
	if n == nil || n.body == nil {
		return fail(types.MakeToolErrorInvalidCommandPath(slices.Clone(inv.commandPath)))
	}
	ce := n.body
	h := e.handlers[ce]
	if h == nil {
		if e.adapter {
			return fail(toolDefinitionError(d))
		}
		return d.passThrough(inv)
	}
	args, terr := ce.decodeArgs(d, inv.input, inv.stdin, inv.principal)
	if terr != nil {
		return fail(*terr)
	}
	inv.undeclare(ce)
	out, err := runWithOutputs(label, inv.outputs(), func() (reflect.Value, error) { return h(inv, args) })
	if err != nil {
		return fail(d.handlerError(ce, err))
	}
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](d.encodeResult(ce, out))
}

// passThrough hands a command a transparent middleware does not handle
// straight to the tool beneath, passing its outputs through.
func (d *definitions) passThrough(inv *middlewareInvocation) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	fail := witTypes.Err[toolCommon.InvocationResult, types.ToolError]
	var result golem.Option[golem.TypedValue]
	_, err := runWithOutputs("middleware "+inv.entry.name, inv.outputs(), func() (reflect.Value, error) {
		started, err := inv.startRaw(inv.input)
		if err != nil {
			return reflect.Value{}, err
		}
		result, err = started.Wait()
		return reflect.Value{}, err
	})
	if err != nil {
		return fail(passThroughToolError(err))
	}
	wire := witTypes.None[types.TypedSchemaValue]()
	if v, has := result.Get(); has {
		wire = witTypes.Some(witOf(v))
	}
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
		Result: wire, Stdout: witTypes.None[*witTypes.StreamReader[uint8]](), Stderr: witTypes.None[*witTypes.StreamReader[uint8]](),
	})
}

// versionOrDefault is the published version of a middleware, 0.0.0 when it
// declares none, as in the other SDKs.
func versionOrDefault(v string) string {
	if v == "" {
		return "0.0.0"
	}
	return v
}
