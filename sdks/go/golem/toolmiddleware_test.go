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
	"fmt"
	"io"
	"strings"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type PolicyParams struct{ Block string }

type AuditParams struct {
	Channel string
	Deny    bool
}

func mustTypedValue[T any](t *testing.T, v T) TypedValue {
	t.Helper()
	tv, err := EncodeTypedValue(v)
	if err != nil {
		t.Fatalf("EncodeTypedValue: %v", err)
	}
	return tv
}

// localUnderlying is the layer beneath a middleware: the wrapped tool's own
// dispatcher, run the way the host runs it.
func localUnderlying(d *definitions, e *toolEntry) underlyingLayer {
	return underlyingLayer{start: func(path []string, input types.TypedSchemaValue, stdin io.Reader) (toolCall, error) {
		n := e.root.find(path)
		wantStdout := n != nil && n.body != nil && n.body.stdout
		return localCall(d, e, path, input, stdin, wantStdout, AnonymousPrincipal{}), nil
	}}
}

type middlewareRun struct {
	path   []string
	input  types.TypedSchemaValue
	params TypedValue
	stdin  string
	under  underlyingLayer
}

// runMiddlewareFor invokes a middleware the way the host would, returning the
// outcome and what it wrote to standard output.
func runMiddlewareFor(t *testing.T, r *toolRegistry, d *definitions, name string, run middlewareRun) (
	witTypes.Result[toolCommon.InvocationResult, types.ToolError], *fakeSink,
) {
	t.Helper()
	e, ok := r.getMiddleware(name)
	if !ok {
		t.Fatalf("no middleware %s", name)
	}
	stdin := &byteReader{absent: absentStdin}
	if run.stdin != "" {
		stdin = &byteReader{src: &fakeSource{items: []streamItem{chunk(run.stdin)}}}
	}
	sink := &fakeSink{}
	inv := &middlewareInvocation{
		toolName:    "vcs",
		parameters:  run.params,
		commandPath: run.path,
		input:       run.input,
		stdin:       stdin,
		stdout:      &ToolStdout{sink: sink},
		principal:   AnonymousPrincipal{},
		under:       run.under,
	}
	return d.runMiddleware(e, inv), sink
}

func resultJSON(t *testing.T, res witTypes.Result[toolCommon.InvocationResult, types.ToolError]) any {
	t.Helper()
	if res.IsErr() {
		t.Fatalf("invocation failed: %+v", res.Err())
	}
	out, err := TypedValue{wit: res.Ok().Result.Some()}.JSON()
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func TestTransparentMiddlewareInterceptsAHandledCommand(t *testing.T) {
	v, r, d := newVcs(t)
	policy := v.tool.Middleware[PolicyParams]("policy", ToolMiddlewareSpec{Version: "1.0.0"})
	_ = policy.Handle(v.commit, func(ctx *ToolMiddlewareContext[PolicyParams], a CommitArgs) (CommitResult, error) {
		if a.Message == ctx.Parameters().Block {
			return CommitResult{}, errors.New("blocked by policy")
		}
		if _, ok := ctx.Principal().(AnonymousPrincipal); !ok {
			return CommitResult{}, errors.New("lost the principal")
		}
		a.Message += " (audited)"
		return policy.Underlying(ctx, v.commit).Forward(a)
	})
	e, _ := r.get("vcs")
	under := localUnderlying(d, e)

	input := encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "fix"; a.Paths = []string{"a"} })
	res, _ := runMiddlewareFor(t, r, d, "policy", middlewareRun{
		path: []string{"commit"}, input: input, params: mustTypedValue(t, PolicyParams{Block: "nope"}), under: under,
	})
	if out := resultJSON(t, res).(map[string]any); out["summary"] != "vcs commit: fix (audited)" {
		t.Errorf("result %v", out)
	}

	blocked := encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "nope"; a.Paths = []string{"a"} })
	res, _ = runMiddlewareFor(t, r, d, "policy", middlewareRun{
		path: []string{"commit"}, input: blocked, params: mustTypedValue(t, PolicyParams{Block: "nope"}), under: under,
	})
	if res.IsOk() || res.Err().Tag() != types.ToolErrorInvalidResult || !strings.Contains(res.Err().InvalidResult(), "blocked by policy") {
		t.Errorf("a blocked call gave %+v", res)
	}

	// A declared error of the tool beneath passes through unchanged.
	empty := encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "empty" })
	res, _ = runMiddlewareFor(t, r, d, "policy", middlewareRun{
		path: []string{"commit"}, input: empty, params: mustTypedValue(t, PolicyParams{}), under: under,
	})
	if res.IsOk() || res.Err().Tag() != types.ToolErrorCustomError || res.Err().CustomError().Name != "nothing-to-commit" {
		t.Errorf("the tool's declared error did not pass through: %+v", res)
	}
}

func TestTransparentMiddlewarePassesUnhandledCommandsThrough(t *testing.T) {
	v, r, d := newVcs(t)
	policy := v.tool.Middleware[PolicyParams]("policy", ToolMiddlewareSpec{})
	_ = policy.Handle(v.commit, func(ctx *ToolMiddlewareContext[PolicyParams], a CommitArgs) (CommitResult, error) {
		return policy.Underlying(ctx, v.commit).Forward(a)
	})
	e, _ := r.get("vcs")
	input := encodeArgs(t, v.push.ce, func(a *PushArgs) { a.Name = "origin" })
	res, sink := runMiddlewareFor(t, r, d, "policy", middlewareRun{
		path: []string{"remote", "push"}, input: input, params: mustTypedValue(t, PolicyParams{}), stdin: "abc", under: localUnderlying(d, e),
	})
	if out := resultJSON(t, res); fmt.Sprint(out) != "33" {
		t.Errorf("result %v", out)
	}
	if string(sink.written) != "ABC" || !sink.finished {
		t.Errorf("stdout %q finished=%v", sink.written, sink.finished)
	}

	// The tool fails with a declared error, which also fails its stdout; the
	// declared error is what the caller sees, not the stream failure.
	forbidden := encodeArgs(t, v.push.ce, func(a *PushArgs) { a.Name = "forbidden" })
	res, sink = runMiddlewareFor(t, r, d, "policy", middlewareRun{
		path: []string{"remote", "push"}, input: forbidden, params: mustTypedValue(t, PolicyParams{}), under: localUnderlying(d, e),
	})
	if res.IsOk() || res.Err().Tag() != types.ToolErrorCustomError || res.Err().CustomError().Name != "rejected" {
		t.Errorf("a declared error beneath gave %+v", res)
	}
	if sink.failed == nil {
		t.Error("the middleware's stdout was not failed")
	}
}

func TestStdoutMiddlewareForwardsAndRewritesOutput(t *testing.T) {
	v, r, d := newVcs(t)
	e, _ := r.get("vcs")
	quiet := v.tool.Middleware[Unit]("forward", ToolMiddlewareSpec{})
	_ = quiet.HandleStdout(v.push, func(ctx *ToolMiddlewareStdoutContext[Unit], a PushArgs) (int32, error) {
		return quiet.UnderlyingStdout(ctx, v.push).Forward(a)
	})
	prefix := v.tool.Middleware[Unit]("prefix", ToolMiddlewareSpec{})
	_ = prefix.HandleStdout(v.push, func(ctx *ToolMiddlewareStdoutContext[Unit], a PushArgs) (int32, error) {
		inv, err := prefix.UnderlyingStdout(ctx, v.push).Start(a)
		if err != nil {
			return 0, err
		}
		data, err := io.ReadAll(inv.Stdout())
		if err != nil {
			return 0, err
		}
		if _, err := io.WriteString(ctx.Stdout(), "> "+string(data)); err != nil {
			return 0, err
		}
		return inv.Wait()
	})

	input := encodeArgs(t, v.push.ce, func(a *PushArgs) { a.Name = "origin" })
	for name, want := range map[string]string{"forward": "HELLO", "prefix": "> HELLO"} {
		res, sink := runMiddlewareFor(t, r, d, name, middlewareRun{
			path: []string{"remote", "push"}, input: input, params: mustTypedValue(t, Unit{}), stdin: "hello", under: localUnderlying(d, e),
		})
		if res.IsErr() {
			t.Fatalf("%s: %+v", name, res.Err())
		}
		if string(sink.written) != want || !sink.finished {
			t.Errorf("%s: stdout %q finished=%v", name, sink.written, sink.finished)
		}
	}
}

type V2 struct{}

type SaveArgs struct{ Text string }

func TestAdapterPresentsOneToolOverAnother(t *testing.T) {
	v, r, d := newVcs(t)
	v2 := defineToolInto[V2](r, d, "vcs2", ToolSpec{Version: "2.0.0"}, true)
	save := v2.Command[SaveArgs, string]("save", func(a *SaveArgs, s *ToolCommandSpec) { s.Positional(&a.Text) })
	adapter := v2.Adapter[Unit]("vcs2-on-vcs", v.tool, ToolMiddlewareSpec{})
	_ = adapter.Handle(save, func(ctx *ToolMiddlewareContext[Unit], a SaveArgs) (string, error) {
		res, err := adapter.Underlying(ctx, v.commit).Call(func(b *CommitArgs) {
			b.Message = a.Text
			b.Paths = []string{"all"}
		})
		return res.Summary, err
	})
	e, _ := r.get("vcs")
	input := encodeArgs(t, save.ce, func(a *SaveArgs) { a.Text = "snapshot" })
	res, _ := runMiddlewareFor(t, r, d, "vcs2-on-vcs", middlewareRun{
		path: []string{"save"}, input: input, params: mustTypedValue(t, Unit{}), under: localUnderlying(d, e),
	})
	if out := resultJSON(t, res); out != "vcs commit: snapshot" {
		t.Errorf("result %v", out)
	}

	m, _ := r.getMiddleware("vcs2-on-vcs")
	built, ok := d.buildToolMiddleware(m)
	if !ok {
		t.Fatalf("metadata: %s", allDefErrors(d.errs))
	}
	scope := built.Scope.Monomorphic()
	if scope.Presented.Version != "2.0.0" || scope.Expected.IsNone() || scope.Expected.Some().Version != "1.2.0" {
		t.Errorf("scope %+v", scope)
	}
}

func TestAdapterMustHandleEveryCommand(t *testing.T) {
	v, r, d := newVcs(t)
	v2 := defineToolInto[V2](r, d, "vcs2", ToolSpec{}, true)
	save := v2.Command[SaveArgs, string]("save", func(a *SaveArgs, s *ToolCommandSpec) { s.Positional(&a.Text) })
	v2.Command[SaveArgs, string]("load", func(a *SaveArgs, s *ToolCommandSpec) { s.Positional(&a.Text) })
	adapter := v2.Adapter[Unit]("partial", v.tool, ToolMiddlewareSpec{})
	_ = adapter.Handle(save, func(*ToolMiddlewareContext[Unit], SaveArgs) (string, error) { return "", nil })
	r.discoverMiddlewares(d)
	mustDefErr(t, d, "does not handle its command load")
}

func TestUniversalMiddlewareWrapsAnyTool(t *testing.T) {
	v, r, d := newVcs(t)
	var seen []string
	audit := defineUniversalToolMiddlewareInto[AuditParams](r, d, "audit", ToolMiddlewareSpec{})
	_ = audit.Handle(func(ctx *UniversalToolMiddlewareContext[AuditParams]) (Option[TypedValue], error) {
		seen = append(seen, ctx.Parameters().Channel+":"+ctx.ToolName()+":"+strings.Join(ctx.CommandPath(), " "))
		if ctx.Parameters().Deny {
			return None[TypedValue](), errors.New("denied by audit policy")
		}
		return ctx.Next(ctx.Input())
	})
	e, _ := r.get("vcs")
	under := localUnderlying(d, e)

	input := encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "m"; a.Paths = []string{"x"} })
	res, _ := runMiddlewareFor(t, r, d, "audit", middlewareRun{
		path: []string{"commit"}, input: input, params: mustTypedValue(t, AuditParams{Channel: "ops"}), under: under,
	})
	if out := resultJSON(t, res).(map[string]any); out["summary"] != "vcs commit: m" {
		t.Errorf("result %v", out)
	}
	if len(seen) != 1 || seen[0] != "ops:vcs:commit" {
		t.Errorf("seen %v", seen)
	}

	push := encodeArgs(t, v.push.ce, func(a *PushArgs) { a.Name = "origin" })
	res, sink := runMiddlewareFor(t, r, d, "audit", middlewareRun{
		path: []string{"remote", "push"}, input: push, params: mustTypedValue(t, AuditParams{}), stdin: "hi", under: under,
	})
	if res.IsErr() || string(sink.written) != "HI" {
		t.Errorf("stdout through a universal middleware: %q, %+v", sink.written, res)
	}

	res, _ = runMiddlewareFor(t, r, d, "audit", middlewareRun{
		path: []string{"commit"}, input: input, params: mustTypedValue(t, AuditParams{Deny: true}), under: under,
	})
	if res.IsOk() || !strings.Contains(res.Err().InvalidResult(), "denied by audit policy") {
		t.Errorf("denied call gave %+v", res)
	}

	empty := encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "empty" })
	res, _ = runMiddlewareFor(t, r, d, "audit", middlewareRun{
		path: []string{"commit"}, input: empty, params: mustTypedValue(t, AuditParams{}), under: under,
	})
	if res.IsOk() || res.Err().Tag() != types.ToolErrorCustomError {
		t.Errorf("the tool's own error did not pass through: %+v", res)
	}
}

func TestMiddlewareMetadata(t *testing.T) {
	v, r, d := newVcs(t)
	policy := v.tool.Middleware[PolicyParams]("policy", ToolMiddlewareSpec{Version: "2.0.0", Summary: "Caps", Aliases: []string{"p"}})
	_ = policy.Handle(v.commit, func(ctx *ToolMiddlewareContext[PolicyParams], a CommitArgs) (CommitResult, error) {
		return policy.Underlying(ctx, v.commit).Forward(a)
	})
	audit := defineUniversalToolMiddlewareInto[AuditParams](r, d, "audit", ToolMiddlewareSpec{})
	_ = audit.Handle(func(ctx *UniversalToolMiddlewareContext[AuditParams]) (Option[TypedValue], error) {
		return ctx.Next(ctx.Input())
	})
	found, ok := r.discoverMiddlewares(d)
	if !ok || len(found) != 2 {
		t.Fatalf("discovery: %v %s", ok, allDefErrors(d.errs))
	}
	typed, universal := found[0], found[1]
	if typed.Name != "policy" || typed.Version != "2.0.0" || typed.Doc.Summary != "Caps" ||
		typed.Scope.Tag() != toolCommon.ToolMiddlewareScopeMonomorphic {
		t.Errorf("typed middleware: %+v", typed)
	}
	if universal.Scope.Tag() != toolCommon.ToolMiddlewareScopeUniversal {
		t.Errorf("universal scope tag %d", universal.Scope.Tag())
	}
	root := universal.ParameterSchema.TypeNodes[universal.ParameterSchema.Root]
	if root.Body.Tag() != types.SchemaTypeBodyRecordType {
		t.Errorf("parameter schema root tag %d", root.Body.Tag())
	}
}

type Twin struct{}

func TestMiddlewareDeclarationErrors(t *testing.T) {
	t.Run("universal without a handler", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		defineUniversalToolMiddlewareInto[Unit](r, d, "bare", ToolMiddlewareSpec{})
		r.discoverMiddlewares(d)
		mustDefErr(t, d, "has no handler")
	})
	t.Run("typed without a handler", func(t *testing.T) {
		v, r, d := newVcs(t)
		v.tool.Middleware[Unit]("idle", ToolMiddlewareSpec{})
		r.discoverMiddlewares(d)
		mustDefErr(t, d, "handles no command")
	})
	t.Run("duplicate", func(t *testing.T) {
		r, d := newToolRegistry(), newDefinitions()
		defineUniversalToolMiddlewareInto[Unit](r, d, "dup", ToolMiddlewareSpec{})
		defineUniversalToolMiddlewareInto[Unit](r, d, "dup", ToolMiddlewareSpec{})
		mustDefErr(t, d, "already defined")
	})
	t.Run("a command handled twice", func(t *testing.T) {
		v, _, d := newVcs(t)
		m := v.tool.Middleware[Unit]("twice", ToolMiddlewareSpec{})
		h := func(*ToolMiddlewareContext[Unit], CommitArgs) (CommitResult, error) { return CommitResult{}, nil }
		_ = m.Handle(v.commit, h)
		_ = m.Handle(v.commit, h)
		mustDefErr(t, d, "handles command commit twice")
	})
	t.Run("a command of another tool sharing the identity type", func(t *testing.T) {
		_, r, d := newVcs(t)
		a := defineToolInto[Twin](r, d, "a", ToolSpec{}, true)
		b := defineToolInto[Twin](r, d, "b", ToolSpec{}, true)
		cmdB := b.Command[SaveArgs, string]("save", func(x *SaveArgs, s *ToolCommandSpec) { s.Positional(&x.Text) })
		m := a.Middleware[Unit]("mixed", ToolMiddlewareSpec{})
		_ = m.Handle(cmdB, func(*ToolMiddlewareContext[Unit], SaveArgs) (string, error) { return "", nil })
		mustDefErr(t, d, "which it does not present")
	})
}

// TestMiddlewareRelaysStderrBeneath — a middleware has no stderr API yet, so
// the layer beneath's standard error reaches the caller through it unchanged,
// and is released when the middleware has none of its own.
func TestMiddlewareRelaysStderrBeneath(t *testing.T) {
	_, r, d := newVcs(t)
	audit := defineUniversalToolMiddlewareInto[AuditParams](r, d, "audit", ToolMiddlewareSpec{})
	_ = audit.Handle(func(ctx *UniversalToolMiddlewareContext[AuditParams]) (Option[TypedValue], error) {
		return ctx.Next(ctx.Input())
	})
	e, _ := r.getMiddleware("audit")
	released := false
	under := underlyingLayer{start: func([]string, types.TypedSchemaValue, io.Reader) (toolCall, error) {
		return toolCall{
			stderr: &byteReader{
				src:     &fakeSource{items: []streamItem{chunk("warn")}},
				release: func() { released = true },
			},
			wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
				return witTypes.None[types.TypedSchemaValue](), nil
			},
			cancel: func() {},
		}, nil
	}}
	run := func(stderr *ToolStdout) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
		return d.runMiddleware(e, &middlewareInvocation{
			toolName: "vcs", commandPath: []string{"commit"},
			parameters: mustTypedValue(t, AuditParams{}),
			stdin:      &byteReader{absent: absentStdin},
			stdout:     &ToolStdout{absent: absentStdout},
			stderr:     stderr,
			principal:  AnonymousPrincipal{},
			under:      under,
		})
	}

	sink := &fakeSink{}
	if res := run(&ToolStdout{sink: sink}); res.IsErr() {
		t.Fatalf("relaying stderr failed: %+v", res.Err())
	}
	if string(sink.written) != "warn" || !sink.finished || released {
		t.Errorf("stderr %q finished=%v released=%v", sink.written, sink.finished, released)
	}

	if res := run(&ToolStdout{absent: absentStdout}); res.IsErr() {
		t.Fatalf("without a stderr of its own: %+v", res.Err())
	}
	if !released {
		t.Errorf("the stderr beneath was not released")
	}
}
