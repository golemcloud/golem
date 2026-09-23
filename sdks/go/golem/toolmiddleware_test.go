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
	"errors"
	"strings"
	"testing"

	mwExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_tool_middleware_guest"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type AuditParams struct{ Channel string }

// fakeLayer stands in for the layer beneath a middleware, recording what it was
// handed and replaying a scripted outcome.
type fakeLayer struct {
	calls   int
	gotPath []string
	gotJSON any
	outcome MiddlewareOutcome
}

func (f *fakeLayer) invoke(commandPath []string, input TypedValue, _ nextStdin) MiddlewareOutcome {
	f.calls++
	f.gotPath = commandPath
	f.gotJSON, _ = input.JSON()
	return f.outcome
}

func mustTypedValue[T any](t *testing.T, v T) TypedValue {
	t.Helper()
	tv, err := EncodeTypedValue(v)
	if err != nil {
		t.Fatalf("EncodeTypedValue: %v", err)
	}
	return tv
}

// runMiddleware registers a middleware and invokes it against a fake layer.
func runMiddleware(
	t *testing.T, h func(*MiddlewareContext[AuditParams]) MiddlewareOutcome, next nextLayer, input TypedValue,
) (witTypes.Result[toolCommon.InvocationResult, types.ToolError], *definitions) {
	t.Helper()
	saved := toolDefs
	toolDefs = newToolRegistry()
	t.Cleanup(func() { toolDefs = saved })

	d := newDefinitions()
	m := defineToolMiddlewareInto[AuditParams](toolDefs, d, "audit", ToolMiddlewareSpec{
		Version: "1.0.0", Summary: "Records every invocation",
	}, nil)
	handleToolMiddlewareInto(toolDefs, d, m, h)

	got := d.invokeMiddleware(&middlewareCall{
		middleware:  "audit",
		toolName:    "greeter",
		parameters:  mustTypedValue(t, AuditParams{Channel: "ops"}),
		commandPath: []string{"greet"},
		input:       input,
		stdout:      newToolStdout(mwExports.Stdout{}),
		next:        next,
	})
	return got, d
}

// TestMiddlewareWrapsTheCallAndPassesItOn — the common shape: observe, forward
// unchanged, return what came back.
func TestMiddlewareWrapsTheCallAndPassesItOn(t *testing.T) {
	layer := &fakeLayer{outcome: Succeed(mustTypedValue(t, "hi ada"))}
	var sawChannel, sawTool string
	var sawPath []string

	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		sawChannel, sawTool, sawPath = ctx.Parameters().Channel, ctx.ToolName(), ctx.CommandPath()
		return ctx.Next(ctx.Input())
	}, layer, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	if sawChannel != "ops" || sawTool != "greeter" || strings.Join(sawPath, " ") != "greet" {
		t.Errorf("context carried %q/%q/%v", sawChannel, sawTool, sawPath)
	}
	if layer.calls != 1 || layer.gotJSON != "ada" {
		t.Errorf("layer saw %d calls with %v", layer.calls, layer.gotJSON)
	}
	out, err := TypedValue{wit: got.Ok().Result.Some()}.JSON()
	if err != nil || out != "hi ada" {
		t.Errorf("result %v (%v), want hi ada", out, err)
	}
}

// TestMiddlewareRewritesInputAndResult — a middleware has no Go types for the
// tools it wraps, so it works through the schema that travels with the value.
func TestMiddlewareRewritesInputAndResult(t *testing.T) {
	layer := &fakeLayer{outcome: Succeed(mustTypedValue(t, "hi ADA"))}

	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		name, err := ctx.Input().JSON()
		if err != nil {
			return Fail(err)
		}
		rewritten, err := ctx.Input().WithJSON(strings.ToUpper(name.(string)))
		if err != nil {
			return Fail(err)
		}
		outcome := ctx.Next(rewritten)
		result, ok := outcome.Result()
		if !ok {
			return outcome
		}
		text, err := result.JSON()
		if err != nil {
			return Fail(err)
		}
		replaced, err := result.WithJSON(text.(string) + "!")
		if err != nil {
			return Fail(err)
		}
		return outcome.WithResult(replaced)
	}, layer, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	if layer.gotJSON != "ADA" {
		t.Errorf("layer saw %v, want the rewritten input", layer.gotJSON)
	}
	out, _ := TypedValue{wit: got.Ok().Result.Some()}.JSON()
	if out != "hi ADA!" {
		t.Errorf("result %v, want the rewritten result", out)
	}
}

// TestMiddlewareCanShortCircuit — not calling Next is how a middleware denies
// or answers a call itself.
func TestMiddlewareCanShortCircuit(t *testing.T) {
	layer := &fakeLayer{outcome: Succeed(mustTypedValue(t, "unreachable"))}

	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		return Succeed(mustTypedValue(t, "cached"))
	}, layer, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	if layer.calls != 0 {
		t.Error("short-circuiting still reached the layer beneath")
	}
	out, _ := TypedValue{wit: got.Ok().Result.Some()}.JSON()
	if out != "cached" {
		t.Errorf("result %v, want cached", out)
	}
}

// TestMiddlewarePassesTheToolsOwnErrorThrough — a caller should see the tool's
// error, not a middleware's paraphrase of it.
func TestMiddlewarePassesTheToolsOwnErrorThrough(t *testing.T) {
	inner := types.MakeToolErrorCustomError(types.CustomToolError{Name: "not-found"})
	layer := &fakeLayer{outcome: MiddlewareOutcome{
		err: &UnderlyingError{Kind: "reported an error", ToolError: &inner},
	}}

	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		return ctx.Next(ctx.Input())
	}, layer, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultErr {
		t.Fatal("a failing inner layer produced a successful invocation")
	}
	if got.Err().Tag() != types.ToolErrorCustomError {
		t.Fatalf("error tag %d, want the tool's own custom-error", got.Err().Tag())
	}
	if name := got.Err().CustomError().Name; name != "not-found" {
		t.Errorf("error name %q, want not-found", name)
	}
}

// TestMiddlewareReportsRuntimeRefusals — a denial or cancellation is the
// runtime's, not the tool's, and has to stay distinguishable.
func TestMiddlewareReportsRuntimeRefusals(t *testing.T) {
	layer := &fakeLayer{outcome: MiddlewareOutcome{
		err: &UnderlyingError{Kind: "denied", Message: "quota"},
	}}

	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		return ctx.Next(ctx.Input())
	}, layer, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultErr {
		t.Fatal("a denied call produced a successful invocation")
	}
	if msg := got.Err().InvalidResult(); !strings.Contains(msg, "denied") {
		t.Errorf("message %q does not report the denial", msg)
	}
}

func TestMiddlewareWithoutALayerBeneathSaysSo(t *testing.T) {
	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		return ctx.Next(ctx.Input())
	}, absentNextLayer{}, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultErr {
		t.Fatal("calling an absent layer succeeded")
	}
	if msg := got.Err().InvalidResult(); !strings.Contains(msg, "without a layer beneath") {
		t.Errorf("message %q", msg)
	}
}

func TestMiddlewarePanicBecomesAToolError(t *testing.T) {
	got, _ := runMiddleware(t, func(ctx *MiddlewareContext[AuditParams]) MiddlewareOutcome {
		panic("middleware gave up")
	}, absentNextLayer{}, mustTypedValue(t, "ada"))

	if got.Tag() != witTypes.ResultErr {
		t.Fatal("a panicking middleware produced a successful invocation")
	}
	if msg := got.Err().InvalidResult(); !strings.Contains(msg, "middleware gave up") {
		t.Errorf("message %q lost the panic", msg)
	}
}

// TestMiddlewareMetadataPublishesItsParameterSchema — an installation's
// configuration can be checked before the middleware ever runs.
func TestMiddlewareMetadataPublishesItsParameterSchema(t *testing.T) {
	saved := toolDefs
	toolDefs = newToolRegistry()
	t.Cleanup(func() { toolDefs = saved })

	d := newDefinitions()
	m := defineToolMiddlewareInto[AuditParams](toolDefs, d, "audit", ToolMiddlewareSpec{
		Version: "2.0.0", Summary: "Records every invocation", Aliases: []string{"log"},
	}, nil)
	handleToolMiddlewareInto(toolDefs, d, m, func(*MiddlewareContext[AuditParams]) MiddlewareOutcome {
		return SucceedWithNothing()
	})

	found, ok := toolDefs.discoverMiddlewares(d)
	if !ok {
		t.Fatalf("discovery failed: %s", allDefErrors(d.errs))
	}
	if len(found) != 1 {
		t.Fatalf("discovered %d middlewares, want 1", len(found))
	}
	got := found[0]
	if got.Name != "audit" || got.Version != "2.0.0" || got.Doc.Summary != "Records every invocation" {
		t.Errorf("metadata is %+v", got)
	}
	if len(got.Aliases) != 1 || got.Aliases[0] != "log" {
		t.Errorf("aliases %v, want [log]", got.Aliases)
	}
	// No Wraps means it applies to any tool.
	if got.Scope.Tag() != toolCommon.ToolMiddlewareScopeUniversal {
		t.Errorf("scope tag %d, want universal", got.Scope.Tag())
	}
	root := got.ParameterSchema.TypeNodes[got.ParameterSchema.Root]
	if root.Body.Tag() != types.SchemaTypeBodyRecordType {
		t.Errorf("parameter schema root tag %d, want record", root.Body.Tag())
	}
}

func TestMiddlewareDeclarationErrors(t *testing.T) {
	t.Run("missing handler", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		defineToolMiddlewareInto[AuditParams](r, d, "bare", ToolMiddlewareSpec{}, nil)
		r.discoverMiddlewares(d)
		mustDefErr(t, d, "has no handler")
	})

	t.Run("duplicate", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		defineToolMiddlewareInto[AuditParams](r, d, "dup", ToolMiddlewareSpec{}, nil)
		defineToolMiddlewareInto[AuditParams](r, d, "dup", ToolMiddlewareSpec{}, nil)
		mustDefErr(t, d, "already defined")
	})

	t.Run("half a scope", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "greeter", ToolSpec{})
		defineToolMiddlewareInto[AuditParams](r, d, "half", ToolMiddlewareSpec{},
			[]ToolMiddlewareOpt{func(o *middlewareOpts) { o.presented = def }})
		mustDefErr(t, d, "needs both a presented and an expected tool")
	})
}

// TestUnderlyingErrorsAreDistinguishable — a middleware may need to tell a
// denial from a cancellation from the tool's own failure.
func TestUnderlyingErrorsAreDistinguishable(t *testing.T) {
	var ue *UnderlyingError
	err := error(&UnderlyingError{Kind: "cancelled"})
	if !errors.As(err, &ue) || ue.Kind != "cancelled" {
		t.Fatalf("errors.As did not recover the kind from %v", err)
	}
	if ue.ToolError != nil {
		t.Error("a cancellation carried a tool error")
	}
	if got := err.Error(); got != "golem: underlying tool cancelled" {
		t.Errorf("message %q", got)
	}
}
