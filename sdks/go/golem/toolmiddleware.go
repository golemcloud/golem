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
	"fmt"
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
//	var Policy = inventory.Tool.Middleware[PolicyParams]("inventory-policy", golem.ToolMiddlewareSpec{Version: "1.0.0"})
//
//	var _ = Policy.Handle(inventory.Adjust, func(ctx *golem.ToolMiddlewareContext[PolicyParams], a inventory.AdjustArgs) (int32, error) {
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

// ToolMiddlewareSpec describes a middleware as a whole.
type ToolMiddlewareSpec struct {
	Version     string
	Summary     string
	Description string
	Aliases     []string
}

// middlewareEntry is one registered middleware.
type middlewareEntry struct {
	name       string
	spec       ToolMiddlewareSpec
	paramsType reflect.Type
	d          *definitions

	// presented and wrapped are the tools of a typed middleware, the same
	// tool for a transparent one; nil for a universal one.
	presented *toolEntry
	wrapped   *toolEntry
	adapter   bool
	handlers  map[*commandEntry]middlewareHandler

	// universal is the handler of a universal middleware.
	universal func(*middlewareInvocation) (Option[TypedValue], error)
}

type middlewareHandler func(*middlewareInvocation, reflect.Value) (reflect.Value, error)

func (e *middlewareEntry) fail(format string, args ...any) {
	e.d.recordErr("", "", "tool middleware %s: %s", e.name, fmt.Sprintf(format, args...))
}

func registerMiddleware(r *toolRegistry, d *definitions, e *middlewareEntry) {
	switch {
	case e.name == "":
		d.recordErr("", "", "a tool middleware requires a name")
	case r.middlewaresByName[e.name] != nil:
		d.recordErr("", "", "tool middleware already defined: %s", e.name)
	default:
		r.middlewareOrder = append(r.middlewareOrder, e.name)
		r.middlewaresByName[e.name] = e
	}
}

// ToolMiddleware is a typed middleware presenting tool T, wrapping tool W and
// configured by P.
type ToolMiddleware[T any, P any, W any] struct{ e *middlewareEntry }

// Name returns the middleware's name.
func (m *ToolMiddleware[T, P, W]) Name() string { return m.e.name }

// Middleware declares a transparent middleware on this tool: it presents and
// wraps the tool, and a command it does not handle goes straight to the tool.
func (t *ToolDefinition[T]) Middleware[P any](name string, spec ToolMiddlewareSpec) *ToolMiddleware[T, P, T] {
	return &ToolMiddleware[T, P, T]{e: newTypedMiddleware[P](t.entry, t.entry, name, spec, false)}
}

// Adapter declares a middleware presenting this tool over wraps: it handles
// every command of this tool, calling the commands of wraps beneath.
func (t *ToolDefinition[T]) Adapter[P any, W any](name string, wraps *ToolDefinition[W], spec ToolMiddlewareSpec) *ToolMiddleware[T, P, W] {
	return &ToolMiddleware[T, P, W]{e: newTypedMiddleware[P](t.entry, wraps.entry, name, spec, true)}
}

func newTypedMiddleware[P any](presented, wrapped *toolEntry, name string, spec ToolMiddlewareSpec, adapter bool) *middlewareEntry {
	e := &middlewareEntry{
		name: name, spec: spec, paramsType: reflect.TypeFor[P](), d: presented.d,
		presented: presented, wrapped: wrapped, adapter: adapter,
		handlers: map[*commandEntry]middlewareHandler{},
	}
	registerMiddleware(presented.r, presented.d, e)
	return e
}

func (e *middlewareEntry) setHandler(ce *commandEntry, h middlewareHandler) Registered {
	switch {
	case ce.node.entry != e.presented:
		e.fail("handles %s of tool %s, which it does not present", ce.label(), ce.node.entry.name)
	case e.handlers[ce] != nil:
		e.fail("handles command %s twice", ce.label())
	default:
		e.handlers[ce] = h
	}
	return Registered{}
}

// Handle intercepts a command of the presented tool. The handler returns the
// command's result as the tool would; a declared error is returned with the
// case's New, and a failure of the layer beneath is passed on unchanged by
// returning it.
func (m *ToolMiddleware[T, P, W]) Handle[A any, O any](
	cmd *ToolCommand[T, A, O], h func(*ToolMiddlewareContext[P], A) (O, error),
) Registered {
	return m.e.setHandler(cmd.ce, func(inv *middlewareInvocation, args reflect.Value) (reflect.Value, error) {
		ctx, err := newMiddlewareContext[P](inv)
		if err != nil {
			return reflect.Value{}, err
		}
		out, err := h(ctx, args.Interface().(A))
		return reflect.ValueOf(&out).Elem(), err
	})
}

// HandleStdout intercepts a command of the presented tool that writes standard
// output, which the handler writes through [ToolMiddlewareStdoutContext.Stdout].
func (m *ToolMiddleware[T, P, W]) HandleStdout[A any, O any](
	cmd *ToolStdoutCommand[T, A, O], h func(*ToolMiddlewareStdoutContext[P], A) (O, error),
) Registered {
	return m.e.setHandler(cmd.ce, func(inv *middlewareInvocation, args reflect.Value) (reflect.Value, error) {
		ctx, err := newMiddlewareContext[P](inv)
		if err != nil {
			return reflect.Value{}, err
		}
		out, err := h(&ToolMiddlewareStdoutContext[P]{ToolMiddlewareContext: ctx}, args.Interface().(A))
		return reflect.ValueOf(&out).Elem(), err
	})
}

// Underlying reaches a command of the wrapped tool for this invocation.
func (m *ToolMiddleware[T, P, W]) Underlying[A any, O any](ctx ToolMiddlewareCall, cmd *ToolCommand[W, A, O]) *ToolUnderlying[A, O] {
	return &ToolUnderlying[A, O]{inv: ctx.middlewareInvocation(), ce: cmd.ce, m: m.e}
}

// UnderlyingStdout reaches a command of the wrapped tool that writes standard
// output.
func (m *ToolMiddleware[T, P, W]) UnderlyingStdout[A any, O any](ctx ToolMiddlewareCall, cmd *ToolStdoutCommand[W, A, O]) *ToolUnderlyingStdout[A, O] {
	return &ToolUnderlyingStdout[A, O]{inv: ctx.middlewareInvocation(), ce: cmd.ce, m: m.e}
}

// ToolMiddlewareCall is the context of a running middleware handler, which is
// what reaching the layer beneath needs.
type ToolMiddlewareCall interface {
	middlewareInvocation() *middlewareInvocation
}

// ToolMiddlewareContext is the per-invocation context of a typed middleware.
type ToolMiddlewareContext[P any] struct {
	inv    *middlewareInvocation
	params P
}

func newMiddlewareContext[P any](inv *middlewareInvocation) (*ToolMiddlewareContext[P], error) {
	params, err := DecodeTypedValue[P](inv.parameters)
	if err != nil {
		return nil, fmt.Errorf("middleware %s: parameters: %w", inv.entry.name, err)
	}
	return &ToolMiddlewareContext[P]{inv: inv, params: params}, nil
}

func (c *ToolMiddlewareContext[P]) middlewareInvocation() *middlewareInvocation { return c.inv }

// Parameters returns the middleware's configuration for this installation.
func (c *ToolMiddlewareContext[P]) Parameters() P { return c.params }

// Principal returns who invoked the tool.
func (c *ToolMiddlewareContext[P]) Principal() Principal { return c.inv.principal }

// Name returns the middleware's own name.
func (c *ToolMiddlewareContext[P]) Name() string { return c.inv.entry.name }

// ToolName returns the name the tool was invoked by.
func (c *ToolMiddlewareContext[P]) ToolName() string { return c.inv.toolName }

// CommandPath returns the command being invoked, from the tool's root.
func (c *ToolMiddlewareContext[P]) CommandPath() []string { return slices.Clone(c.inv.commandPath) }

// ToolMiddlewareStdoutContext is the context of a handler for a command that
// writes standard output.
type ToolMiddlewareStdoutContext[P any] struct {
	*ToolMiddlewareContext[P]
}

// Stdout returns the middleware's standard output. It is finished when the
// handler succeeds and failed when it returns an error or panics.
func (c *ToolMiddlewareStdoutContext[P]) Stdout() *ToolStdout { return c.inv.stdout }

// ToolUnderlying is a command of the wrapped tool, reached from a middleware
// handler.
type ToolUnderlying[A any, O any] struct {
	inv *middlewareInvocation
	ce  *commandEntry
	m   *middlewareEntry
}

// Call runs the command beneath, starting from its declared defaults.
func (u *ToolUnderlying[A, O]) Call(fill func(*A)) (O, error) {
	return finishUnderlying[O](u.ce, u.inv, u.m, fillArgs(fill))
}

// Forward runs the command beneath with complete arguments, unchanged.
func (u *ToolUnderlying[A, O]) Forward(a A) (O, error) {
	return finishUnderlying[O](u.ce, u.inv, u.m, forwardArgs(a))
}

// ToolUnderlyingStdout is a command of the wrapped tool that writes standard
// output, reached from a middleware handler.
type ToolUnderlyingStdout[A any, O any] struct {
	inv *middlewareInvocation
	ce  *commandEntry
	m   *middlewareEntry
}

// Call starts the command beneath, starting from its declared defaults.
func (u *ToolUnderlyingStdout[A, O]) Call(fill func(*A)) (*ToolInvocation[O], error) {
	return startUnderlying[O](u.ce, u.inv, u.m, fillArgs(fill))
}

// Start starts the command beneath with complete arguments, unchanged.
func (u *ToolUnderlyingStdout[A, O]) Start(a A) (*ToolInvocation[O], error) {
	return startUnderlying[O](u.ce, u.inv, u.m, forwardArgs(a))
}

// Forward runs the command beneath, writing its output to the middleware's
// own, and returns its result.
func (u *ToolUnderlyingStdout[A, O]) Forward(a A) (O, error) {
	var zero O
	inv, err := u.Start(a)
	if err != nil {
		return zero, err
	}
	if _, err := io.Copy(u.inv.stdout, inv.Stdout()); err != nil {
		inv.Cancel()
		return zero, err
	}
	return inv.Wait()
}

func forwardArgs[A any](a A) func(reflect.Value) {
	return func(v reflect.Value) { v.Set(reflect.ValueOf(&a).Elem()) }
}

func startUnderlying[O any](ce *commandEntry, inv *middlewareInvocation, m *middlewareEntry, fill func(reflect.Value)) (*ToolInvocation[O], error) {
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
	return commandInvocation[O](ce, inv.toolName, call), nil
}

func finishUnderlying[O any](ce *commandEntry, inv *middlewareInvocation, m *middlewareEntry, fill func(reflect.Value)) (O, error) {
	var zero O
	started, err := startUnderlying[O](ce, inv, m, fill)
	if err != nil {
		return zero, err
	}
	return started.Wait()
}

// UniversalToolMiddleware is a middleware for any tool, configured by P.
type UniversalToolMiddleware[P any] struct{ e *middlewareEntry }

// Name returns the middleware's name.
func (m *UniversalToolMiddleware[P]) Name() string { return m.e.name }

// DefineUniversalToolMiddleware declares a middleware that applies to any tool.
// It has no Go types for the tools it wraps, so the arguments and the result
// travel as [TypedValue]. Call it from a package-level var.
func DefineUniversalToolMiddleware[P any](name string, spec ToolMiddlewareSpec) *UniversalToolMiddleware[P] {
	return defineUniversalToolMiddlewareInto[P](toolDefs, defs, name, spec)
}

func defineUniversalToolMiddlewareInto[P any](r *toolRegistry, d *definitions, name string, spec ToolMiddlewareSpec) *UniversalToolMiddleware[P] {
	e := &middlewareEntry{name: name, spec: spec, paramsType: reflect.TypeFor[P](), d: d}
	registerMiddleware(r, d, e)
	return &UniversalToolMiddleware[P]{e: e}
}

// Handle binds the middleware's implementation. The handler returns the
// result to hand back, none for a command without one; returning a failure of
// the layer beneath passes it on unchanged.
func (m *UniversalToolMiddleware[P]) Handle(h func(*UniversalToolMiddlewareContext[P]) (Option[TypedValue], error)) Registered {
	if m.e.universal != nil {
		m.e.fail("already has a handler")
		return Registered{}
	}
	m.e.universal = func(inv *middlewareInvocation) (Option[TypedValue], error) {
		params, err := DecodeTypedValue[P](inv.parameters)
		if err != nil {
			return None[TypedValue](), fmt.Errorf("middleware %s: parameters: %w", inv.entry.name, err)
		}
		return h(&UniversalToolMiddlewareContext[P]{inv: inv, params: params})
	}
	return Registered{}
}

// UniversalToolMiddlewareContext is the per-invocation context of a universal
// middleware.
type UniversalToolMiddlewareContext[P any] struct {
	inv    *middlewareInvocation
	params P
}

// Parameters returns the middleware's configuration for this installation.
func (c *UniversalToolMiddlewareContext[P]) Parameters() P { return c.params }

// Principal returns who invoked the tool.
func (c *UniversalToolMiddlewareContext[P]) Principal() Principal { return c.inv.principal }

// Name returns the middleware's own name.
func (c *UniversalToolMiddlewareContext[P]) Name() string { return c.inv.entry.name }

// ToolName returns the name the tool was invoked by.
func (c *UniversalToolMiddlewareContext[P]) ToolName() string { return c.inv.toolName }

// ToolMetadata returns the wrapped tool's published metadata, which is how a
// universal middleware learns the shape of a tool it was not written for.
func (c *UniversalToolMiddlewareContext[P]) ToolMetadata() ReflectedTool {
	return newReflectedTool(c.inv.toolName, c.inv.tool)
}

// CommandPath returns the command being invoked, from the tool's root.
func (c *UniversalToolMiddlewareContext[P]) CommandPath() []string {
	return slices.Clone(c.inv.commandPath)
}

// Input returns the call's arguments, the command's canonical input record.
func (c *UniversalToolMiddlewareContext[P]) Input() TypedValue { return TypedValue{wit: c.inv.input} }

// Stdout returns the middleware's standard output. It is finished when the
// handler succeeds and failed when it returns an error or panics.
func (c *UniversalToolMiddlewareContext[P]) Stdout() *ToolStdout { return c.inv.stdout }

// Start hands the call to the layer beneath with the given input and the
// original standard input, and returns the running invocation.
func (c *UniversalToolMiddlewareContext[P]) Start(input TypedValue) (*ToolInvocation[Option[TypedValue]], error) {
	var stdin io.Reader
	if c.inv.stdin.present() {
		stdin = c.inv.stdin
	}
	call, err := c.inv.under.start(c.inv.commandPath, input.wit, stdin)
	if err != nil {
		return nil, err
	}
	tool, path := c.inv.toolName, c.inv.commandPath
	return &ToolInvocation[Option[TypedValue]]{call: call, finish: func(call toolCall) (Option[TypedValue], error) {
		res, rpcErr := call.wait()
		if rpcErr != nil {
			return None[TypedValue](), toolCallErrorFromWit(tool, path, *rpcErr)
		}
		if res.IsNone() {
			return None[TypedValue](), nil
		}
		return Some(TypedValue{wit: res.Some()}), nil
	}}, nil
}

// Next hands the call to the layer beneath, writing its output to the
// middleware's own, and returns its result.
func (c *UniversalToolMiddlewareContext[P]) Next(input TypedValue) (Option[TypedValue], error) {
	inv, err := c.Start(input)
	if err != nil {
		return None[TypedValue](), err
	}
	if err := relayStdout(c.inv.stdout, inv.call.stdout); err != nil {
		inv.Cancel()
		return None[TypedValue](), err
	}
	return inv.Wait()
}

// relayStdout copies the output of the layer beneath into the middleware's
// own, when both exist.
func relayStdout(dst *ToolStdout, src *byteReader) error {
	if src == nil || dst == nil || dst.absent != "" {
		return nil
	}
	_, err := io.Copy(dst, src)
	return err
}

// middlewareInvocation is the state of one middleware invocation.
type middlewareInvocation struct {
	entry       *middlewareEntry
	toolName    string
	tool        toolCommon.Tool
	parameters  TypedValue
	commandPath []string
	input       types.TypedSchemaValue
	stdin       *byteReader
	stdout      *ToolStdout
	principal   Principal
	under       underlyingLayer
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

	g := graphBuilder{d: d}
	paramsRoot := g.node(d.compile(e.paramsType))
	paramsGraph := g.build()
	paramsGraph.Root = paramsRoot
	for typ, why := range g.invalids {
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
		Version:         e.spec.Version,
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
		out, err := runWithStdout(label, inv.stdout, func() (reflect.Value, error) {
			res, err := e.universal(inv)
			return reflect.ValueOf(res), err
		})
		if err != nil {
			return fail(passThroughToolError(err))
		}
		result := witTypes.None[types.TypedSchemaValue]()
		if v, has := out.Interface().(Option[TypedValue]).Get(); has {
			result = witTypes.Some(v.wit)
		}
		return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
			Result: result, Stdout: witTypes.None[*witTypes.StreamReader[uint8]](),
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
	out, err := runWithStdout(label, inv.stdout, func() (reflect.Value, error) { return h(inv, args) })
	if err != nil {
		return fail(d.handlerError(ce, err))
	}
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](d.encodeResult(ce, out))
}

// passThrough hands a command a transparent middleware does not handle
// straight to the tool beneath.
func (d *definitions) passThrough(inv *middlewareInvocation) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	fail := witTypes.Err[toolCommon.InvocationResult, types.ToolError]
	var result Option[types.TypedSchemaValue]
	_, err := runWithStdout("middleware "+inv.entry.name, inv.stdout, func() (reflect.Value, error) {
		var stdin io.Reader
		if inv.stdin.present() {
			stdin = inv.stdin
		}
		call, err := inv.under.start(inv.commandPath, inv.input, stdin)
		if err != nil {
			return reflect.Value{}, err
		}
		if err := relayStdout(inv.stdout, call.stdout); err != nil {
			call.cancel()
			return reflect.Value{}, err
		}
		res, rpcErr := call.wait()
		if rpcErr != nil {
			return reflect.Value{}, toolCallErrorFromWit(inv.toolName, inv.commandPath, *rpcErr)
		}
		result = optionFromWit(res)
		return reflect.Value{}, nil
	})
	if err != nil {
		return fail(passThroughToolError(err))
	}
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
		Result: optionToWit(result), Stdout: witTypes.None[*witTypes.StreamReader[uint8]](),
	})
}
