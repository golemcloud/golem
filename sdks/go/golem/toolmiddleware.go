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
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	underlying "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_underlying"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool middleware.
//
// A middleware wraps one tool invocation: it sees the call on its way in, hands
// it to the next inner layer, and sees the outcome on its way back.
//
//	type AuditParams struct{ Channel string }
//
//	var Audit = golem.DefineToolMiddleware[AuditParams]("audit", golem.ToolMiddlewareSpec{
//	    Version: "1.0.0", Summary: "Records every invocation",
//	})
//
//	var _ = golem.HandleToolMiddleware(Audit, func(ctx *golem.MiddlewareContext[AuditParams]) golem.MiddlewareOutcome {
//	    record(ctx.Parameters().Channel, ctx.ToolName(), ctx.CommandPath())
//	    return ctx.Next(ctx.Input())
//	})
//
// A middleware is generic over the tools it wraps, so it does not have their Go
// types: the call's arguments and its result arrive as [TypedValue], which
// carries its own schema. Returning without calling [MiddlewareContext.Next]
// short-circuits the call.
//
// The middleware's static configuration is the type parameter P, whose schema
// is published so an installation can be checked before it runs. Use [Unit] for
// a middleware that takes none.

// ToolMiddlewareSpec describes a middleware as a whole.
type ToolMiddlewareSpec struct {
	Version     string
	Summary     string
	Description string
	Aliases     []string
}

// ToolMiddlewareOpt customises a middleware declaration.
type ToolMiddlewareOpt func(*middlewareOpts)

type middlewareOpts struct {
	presented *ToolDefinition
	expected  *ToolDefinition
}

// Wraps narrows a middleware to one tool shape: it presents presented to its
// caller and expects expected from the layer beneath. Without it a middleware is
// universal and applies to any tool.
func Wraps(presented *ToolDefinition, expected *ToolDefinition) ToolMiddlewareOpt {
	return func(o *middlewareOpts) { o.presented, o.expected = presented, expected }
}

// ToolMiddlewareDefinition is a registered middleware, returned by
// [DefineToolMiddleware].
type ToolMiddlewareDefinition[P any] struct{ name string }

// Name returns the middleware's canonical name.
func (m *ToolMiddlewareDefinition[P]) Name() string { return m.name }

// middlewareEntry is one registered middleware and its handler.
type middlewareEntry struct {
	name       string
	spec       ToolMiddlewareSpec
	opts       middlewareOpts
	paramsType reflect.Type
	invoke     func(*middlewareCall) MiddlewareOutcome
}

// DefineToolMiddleware registers a middleware. Call it from a package-level var
// so registration happens before the component is invoked.
func DefineToolMiddleware[P any](name string, spec ToolMiddlewareSpec, opts ...ToolMiddlewareOpt) *ToolMiddlewareDefinition[P] {
	return defineToolMiddlewareInto[P](toolDefs, defs, name, spec, opts)
}

func defineToolMiddlewareInto[P any](
	r *toolRegistry, d *definitions, name string, spec ToolMiddlewareSpec, opts []ToolMiddlewareOpt,
) *ToolMiddlewareDefinition[P] {
	m := &ToolMiddlewareDefinition[P]{name: name}
	if name == "" {
		d.recordErr("", "", "DefineToolMiddleware requires a name")
		return m
	}
	if _, dup := r.middlewaresByName[name]; dup {
		d.recordErr("", "", "tool middleware already defined: %s", name)
		return m
	}
	var mo middlewareOpts
	for _, o := range opts {
		o(&mo)
	}
	if (mo.presented == nil) != (mo.expected == nil) {
		d.recordErr("", "", "tool middleware %s: golem.Wraps needs both a presented and an expected tool", name)
	}
	e := &middlewareEntry{name: name, spec: spec, opts: mo, paramsType: reflect.TypeFor[P]()}
	r.middlewareOrder = append(r.middlewareOrder, name)
	r.middlewaresByName[name] = e
	return m
}

// HandleToolMiddleware binds a middleware's implementation.
func HandleToolMiddleware[P any](
	m *ToolMiddlewareDefinition[P], h func(*MiddlewareContext[P]) MiddlewareOutcome,
) Registered {
	return handleToolMiddlewareInto(toolDefs, defs, m, h)
}

func handleToolMiddlewareInto[P any](
	r *toolRegistry, d *definitions, m *ToolMiddlewareDefinition[P],
	h func(*MiddlewareContext[P]) MiddlewareOutcome,
) Registered {
	e := r.middlewaresByName[m.name]
	if e == nil {
		d.recordErr("", "", "handler declared for unregistered tool middleware %q", m.name)
		return Registered{}
	}
	if e.invoke != nil {
		d.recordErr("", "", "tool middleware %s already has a handler", m.name)
		return Registered{}
	}
	e.invoke = func(call *middlewareCall) MiddlewareOutcome {
		params, err := DecodeTypedValue[P](call.parameters)
		if err != nil {
			return failOutcome(fmt.Errorf("middleware %s: parameters: %w", m.name, err))
		}
		return h(&MiddlewareContext[P]{call: call, params: params})
	}
	return Registered{}
}

// middlewareCall is the per-invocation state shared by the context, kept apart
// from the type parameter so the dispatcher can build it without knowing P.
type middlewareCall struct {
	middleware  string
	toolName    string
	tool        toolCommon.Tool
	parameters  TypedValue
	commandPath []string
	input       TypedValue
	stdin       nextStdin
	stdout      *ToolStdout
	next        nextLayer
}

// MiddlewareContext is the per-invocation context handed to a middleware.
type MiddlewareContext[P any] struct {
	call   *middlewareCall
	params P
}

// Parameters returns the middleware's static configuration for this
// installation.
func (c *MiddlewareContext[P]) Parameters() P { return c.params }

// Name returns the middleware's own name.
func (c *MiddlewareContext[P]) Name() string { return c.call.middleware }

// ToolName returns the name of the tool being invoked.
func (c *MiddlewareContext[P]) ToolName() string { return c.call.toolName }

// ToolMetadata returns the wrapped tool's published metadata, which is how a
// universal middleware learns the shape of a tool it was not written for.
func (c *MiddlewareContext[P]) ToolMetadata() toolCommon.Tool { return c.call.tool }

// CommandPath returns the command being invoked, from the tool's root.
func (c *MiddlewareContext[P]) CommandPath() []string {
	return append([]string(nil), c.call.commandPath...)
}

// Input returns the call's arguments. A middleware that does not rewrite them
// passes this straight to [MiddlewareContext.Next].
func (c *MiddlewareContext[P]) Input() TypedValue { return c.call.input }

// Stdout returns the middleware's own output stream, which it may write to
// directly or relay the inner layer's output into.
func (c *MiddlewareContext[P]) Stdout() *ToolStdout { return c.call.stdout }

// Next hands the call to the layer beneath and returns its outcome. The
// original standard input is forwarded as-is. Not calling it short-circuits the
// call, which is how a middleware denies or caches one.
func (c *MiddlewareContext[P]) Next(input TypedValue) MiddlewareOutcome {
	return c.call.next.invoke(c.call.commandPath, input, c.call.stdin)
}

// MiddlewareOutcome is what a middleware returns: the result to hand back, or a
// failure. Build one with [Succeed], [SucceedWithNothing] or [Fail], or pass
// through what [MiddlewareContext.Next] returned.
type MiddlewareOutcome struct {
	result    TypedValue
	hasResult bool
	// stdout is the inner layer's output stream when the outcome came from Next,
	// so a middleware can relay it.
	stdout *ToolStdin
	err    error
}

// Succeed returns a result to the caller.
func Succeed(result TypedValue) MiddlewareOutcome {
	return MiddlewareOutcome{result: result, hasResult: true}
}

// SucceedWithNothing returns success with no value, for a command that has no
// result.
func SucceedWithNothing() MiddlewareOutcome { return MiddlewareOutcome{} }

// Fail returns a failure to the caller.
func Fail(err error) MiddlewareOutcome { return failOutcome(err) }

func failOutcome(err error) MiddlewareOutcome { return MiddlewareOutcome{err: err} }

// Result reports the outcome's value, if it has one.
func (o MiddlewareOutcome) Result() (TypedValue, bool) { return o.result, o.hasResult }

// Err reports the outcome's failure, or nil.
func (o MiddlewareOutcome) Err() error { return o.err }

// Stdout returns the inner layer's output stream when this outcome came from
// [MiddlewareContext.Next] and the inner layer produced one.
func (o MiddlewareOutcome) Stdout() *ToolStdin { return o.stdout }

// WithResult replaces the outcome's value, which is how a middleware rewrites
// what the inner layer returned.
func (o MiddlewareOutcome) WithResult(result TypedValue) MiddlewareOutcome {
	o.result, o.hasResult, o.err = result, true, nil
	return o
}

// UnderlyingError reports a failure from the layer beneath a middleware, which
// distinguishes the tool's own error from the runtime refusing or cancelling
// the call.
type UnderlyingError struct {
	// Kind names the failure as the runtime classified it.
	Kind string
	// Message is the accompanying detail, empty for a bare cancellation.
	Message string
	// ToolError is the wrapped tool's own error, when the failure was one.
	ToolError *types.ToolError
}

func (e *UnderlyingError) Error() string {
	if e.Message == "" {
		return "golem: underlying tool " + e.Kind
	}
	return "golem: underlying tool " + e.Kind + ": " + e.Message
}

// underlyingErrorToGo renders the runtime's failure as a Go error.
//
//nolint:unused // called from toolmiddleware_wasm.go
func underlyingErrorToGo(e underlying.UnderlyingError) error {
	switch e.Tag() {
	case underlying.UnderlyingErrorToolError:
		te := e.ToolError()
		return &UnderlyingError{Kind: "reported an error", ToolError: &te, Message: toolErrorMessage(te)}
	case underlying.UnderlyingErrorProtocolError:
		return &UnderlyingError{Kind: "protocol error", Message: e.ProtocolError()}
	case underlying.UnderlyingErrorDenied:
		return &UnderlyingError{Kind: "denied", Message: e.Denied()}
	case underlying.UnderlyingErrorInternalError:
		return &UnderlyingError{Kind: "internal error", Message: e.InternalError()}
	case underlying.UnderlyingErrorCancelled:
		return &UnderlyingError{Kind: "cancelled"}
	case underlying.UnderlyingErrorResourceExhausted:
		return &UnderlyingError{Kind: "resource exhausted", Message: e.ResourceExhausted()}
	}
	return &UnderlyingError{Kind: "failed"}
}

// toolErrorMessage renders a tool error for use inside another error's text.
//
//nolint:unused // reached only from the wasip1 build's error rendering
func toolErrorMessage(e types.ToolError) string {
	switch e.Tag() {
	case types.ToolErrorInvalidToolName:
		return "unknown tool " + e.InvalidToolName()
	case types.ToolErrorInvalidCommandPath:
		return "unknown command " + commandLabel(e.InvalidCommandPath())
	case types.ToolErrorInvalidInput:
		return e.InvalidInput()
	case types.ToolErrorConstraintViolation:
		return e.ConstraintViolation()
	case types.ToolErrorInvalidResult:
		return e.InvalidResult()
	case types.ToolErrorCustomError:
		return e.CustomError().Name
	}
	return "failed"
}

// buildToolMiddleware derives the metadata the host discovers for one
// middleware.
func (d *definitions) buildToolMiddleware(e *middlewareEntry) (toolCommon.ToolMiddleware, bool) {
	ok := true
	if e.invoke == nil {
		d.recordErr("", "", "tool middleware %s has no handler; call golem.HandleToolMiddleware", e.name)
		ok = false
	}

	g := graphBuilder{d: d}
	paramsRoot := g.node(d.compile(e.paramsType))
	paramsGraph := g.build()
	paramsGraph.Root = paramsRoot
	for typ, why := range g.invalids {
		d.recordErr("", "", "tool middleware %s takes parameters of %s, which cannot be represented: %s",
			e.name, typ, why)
		ok = false
	}

	scope := toolCommon.MakeToolMiddlewareScopeUniversal()
	if e.opts.presented != nil {
		presented, presentedOK := d.buildScopedTool(e, e.opts.presented, "presented")
		expected, expectedOK := d.buildScopedTool(e, e.opts.expected, "expected")
		if presentedOK && expectedOK {
			scope = toolCommon.MakeToolMiddlewareScopeMonomorphic(toolCommon.MonomorphicScope{
				Presented: presented,
				Expected:  witTypes.Some(expected),
			})
		} else {
			ok = false
		}
	}

	return toolCommon.ToolMiddleware{
		Name:            e.name,
		Version:         e.spec.Version,
		Aliases:         append([]string(nil), e.spec.Aliases...),
		Doc:             docOf(e.spec.Summary, e.spec.Description),
		Scope:           scope,
		ParameterSchema: paramsGraph,
	}, ok
}

// buildScopedTool derives the metadata of a tool named in a monomorphic scope.
func (d *definitions) buildScopedTool(e *middlewareEntry, t *ToolDefinition, role string) (toolCommon.Tool, bool) {
	entry, known := toolDefs.get(t.Name())
	if !known {
		d.recordErr("", "", "tool middleware %s names the unregistered tool %q as its %s shape",
			e.name, t.Name(), role)
		return toolCommon.Tool{}, false
	}
	return d.buildTool(entry)
}

// nextStdin is the standard input a middleware forwards to the layer beneath.
// It is carried opaquely: a middleware may pass it on but cannot rebuild it,
// since the wire gives no way to construct a stream from the guest.
type nextStdin struct {
	//nolint:unused // set by the wasip1 build's newNextStdin
	wit any
}

// nextLayer is the layer beneath a middleware. The interface keeps the
// dispatcher's logic testable without a host; the wasm build binds the
// generated resource to it (see toolmiddleware_wasm.go).
type nextLayer interface {
	invoke(commandPath []string, input TypedValue, stdin nextStdin) MiddlewareOutcome
}

// absentNextLayer stands in when there is no layer beneath, so calling Next
// reports that rather than dereferencing nil.
type absentNextLayer struct{}

func (absentNextLayer) invoke([]string, TypedValue, nextStdin) MiddlewareOutcome {
	return failOutcome(fmt.Errorf("golem: this middleware was invoked without a layer beneath it"))
}

// invokeMiddleware runs one middleware layer: decode the parameters, hand the
// call to the handler, and translate its outcome back to the wire.
func (d *definitions) invokeMiddleware(call *middlewareCall) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	e, known := toolDefs.getMiddleware(call.middleware)
	if !known {
		return witTypes.Err[toolCommon.InvocationResult](
			types.MakeToolErrorInvalidToolName(call.middleware))
	}
	if e.invoke == nil {
		return witTypes.Err[toolCommon.InvocationResult](toolDefinitionError(d))
	}

	outcome, err := runMiddlewareHandler(e, call)
	if err != nil {
		return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidResult(err.Error()))
	}
	if outcome.err != nil {
		// A failure the inner layer already expressed as a tool error is passed
		// through unchanged, so the caller sees the tool's own error rather than
		// a middleware's paraphrase of it.
		var ue *UnderlyingError
		if errorsAs(outcome.err, &ue) && ue.ToolError != nil {
			return witTypes.Err[toolCommon.InvocationResult](*ue.ToolError)
		}
		return witTypes.Err[toolCommon.InvocationResult](
			types.MakeToolErrorInvalidResult(outcome.err.Error()))
	}

	result := witTypes.None[types.TypedSchemaValue]()
	if outcome.hasResult {
		result = witTypes.Some(outcome.result.wit)
	}
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
		Result: result,
		Stdout: witTypes.None[*witTypes.StreamReader[uint8]](),
	})
}

// runMiddlewareHandler calls the handler, recovering a panic and selecting the
// output stream's terminal, on the same terms as a command handler.
func runMiddlewareHandler(e *middlewareEntry, call *middlewareCall) (outcome MiddlewareOutcome, err error) {
	finished := false
	defer func() {
		if r := recover(); r != nil {
			_ = call.stdout.Fail(StreamFailed(panicMessage(r)))
			outcome, err = MiddlewareOutcome{}, fmt.Errorf(
				"middleware %s panicked: %s", e.name, panicMessage(r))
			return
		}
		if !finished {
			return
		}
		if ferr := call.stdout.finish(); ferr != nil && err == nil {
			err = ferr
		}
	}()
	outcome = e.invoke(call)
	finished = true
	return outcome, nil
}
