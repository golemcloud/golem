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
	"reflect"
	"strings"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type NotFoundPayload struct{ Name string }

type LookupArgs struct{ Name string }

// declareLookup registers a command with two declared failures: one carrying a
// payload and one carrying none.
type Lookup struct{}

func declareLookup(r *toolRegistry, d *definitions) (*ToolCommand[Lookup, LookupArgs, string], *ToolErrorCase[NotFoundPayload, Lookup]) {
	def := defineToolInto[Lookup](r, d, "lookup", ToolSpec{Version: "1.0.0"}, false)
	notFound := DefineToolError[NotFoundPayload](def, "not-found", ToolErrorSpec{
		Kind: UsageError, ExitCode: 2, Summary: "no such name",
	})
	offline := DefineToolError[Unit](def, "offline", ToolErrorSpec{
		Kind: RuntimeError, ExitCode: 69, Summary: "the directory is unreachable",
	})
	unlisted := DefineToolError[Unit](def, "unlisted", ToolErrorSpec{Kind: RuntimeError})

	cmd := def.Body[LookupArgs, string](func(a *LookupArgs, s *ToolCommandSpec) {
		s.Positional(&a.Name)
		s.Raises(notFound, offline)
	})
	_ = cmd.Handle(func(_ *ToolContext, in LookupArgs) (string, error) {
		switch in.Name {
		case "missing":
			return "", notFound.New(NotFoundPayload(in))
		case "offline":
			return "", offline.New(Unit{})
		case "unlisted":
			return "", unlisted.New(Unit{})
		case "raised-by-panic":
			panic(offline.New(Unit{}))
		case "boom":
			panic("something went wrong")
		}
		return "found " + in.Name, nil
	})
	return cmd, notFound
}

// buildToolFor registers a tool on an isolated definition set and derives its
// metadata, so each test sees only its own declarations.
func buildToolFor(t *testing.T, declare func(r *toolRegistry, d *definitions)) (toolCommon.Tool, *toolRegistry, *definitions) {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	declare(r, d)
	tools, ok := r.discover(d)
	if !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.errs))
	}
	if len(tools) != 1 {
		t.Fatalf("discovered %d tools, want 1", len(tools))
	}
	return tools[0], r, d
}

// encodeArgs renders arguments the way a typed caller sends them.
func encodeArgs[A any](t *testing.T, ce *commandEntry, fill func(*A)) types.TypedSchemaValue {
	t.Helper()
	l, ok := ce.resolve()
	if !ok {
		t.Fatalf("command %s is not well-defined: %s", ce.label(), allDefErrors(ce.node.entry.d.errs))
	}
	args := reflect.New(ce.argsType).Elem()
	args.Set(l.defaults)
	fill(args.Addr().Interface().(*A))
	return l.encode(ce.node.entry.d, args)
}

// TestDeclaredErrorsAreInTheCommandContract — the failures are published with
// the command, so a caller can branch on them without parsing a message.
func TestDeclaredErrorsAreInTheCommandContract(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })
	body := tool.Commands.Nodes[0].Body.Some()

	if len(body.Errors) != 2 {
		t.Fatalf("command publishes %d errors, want 2", len(body.Errors))
	}
	byName := map[string]toolCommon.ErrorCase{}
	for _, e := range body.Errors {
		byName[e.Name] = e
	}

	notFound := byName["not-found"]
	if notFound.Kind != uint8(UsageError) || notFound.ExitCode != 2 {
		t.Errorf("not-found is kind %d exit %d, want usage/2", notFound.Kind, notFound.ExitCode)
	}
	if notFound.Payload.IsNone() {
		t.Fatal("not-found lost its payload type")
	}
	if tag := tool.Schema.TypeNodes[notFound.Payload.Some()].Body.Tag(); tag != types.SchemaTypeBodyRecordType {
		t.Errorf("not-found payload tag %d, want record", tag)
	}

	// A Unit payload means the case carries nothing, not a record with no fields.
	if !byName["offline"].Payload.IsNone() {
		t.Error("offline gained a payload it never declared")
	}
	// Declaring an error case does not publish it; only Raises does.
	if _, listed := byName["unlisted"]; listed {
		t.Error("an error case the command never listed reached its contract")
	}
}

// invokeLookup runs the lookup command the way the host would.
func invokeLookup(t *testing.T, d *definitions, r *toolRegistry, name string) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	t.Helper()
	e, _ := r.get("lookup")
	input := encodeArgs(t, e.root.body, func(a *LookupArgs) { a.Name = name })
	return d.invokeCommand(e, nil, input, nil, &ToolStdout{absent: absentStdout}, nil)
}

func TestDeclaredErrorTravelsAsCustomErrorWithItsPayload(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })

	got := invokeLookup(t, d, r, "missing")
	if got.Tag() != witTypes.ResultErr {
		t.Fatal("raising a declared error produced a successful invocation")
	}
	err := got.Err()
	if err.Tag() != types.ToolErrorCustomError {
		t.Fatalf("error tag %d, want custom-error", err.Tag())
	}
	custom := err.CustomError()
	if custom.Name != "not-found" {
		t.Errorf("error name %q, want not-found", custom.Name)
	}
	payload, perr := TypedValue{wit: custom.Payload}.JSON()
	if perr != nil {
		t.Fatalf("payload is not readable: %v", perr)
	}
	obj, ok := payload.(map[string]any)
	if !ok || obj["name"] != "missing" {
		t.Errorf("payload is %v, want the raised record", payload)
	}
}

func TestDeclaredErrorWithoutAPayload(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })

	got := invokeLookup(t, d, r, "offline")
	if got.Tag() != witTypes.ResultErr {
		t.Fatal("raising a declared error produced a successful invocation")
	}
	custom := got.Err().CustomError()
	if custom.Name != "offline" {
		t.Errorf("error name %q, want offline", custom.Name)
	}
	// The host checks a case without a payload against the empty tuple.
	root := custom.Payload.Graph.TypeNodes[custom.Payload.Graph.Root].Body
	value := custom.Payload.Value.ValueNodes[custom.Payload.Value.Root]
	if root.Tag() != types.SchemaTypeBodyTupleType || len(root.TupleType()) != 0 ||
		value.Tag() != types.SchemaValueNodeTupleValue || len(value.TupleValue()) != 0 {
		t.Errorf("a payload-less case carries %+v / %+v, want the empty tuple", root, value)
	}
}

// TestADeclaredErrorMayBePanicked — the typed abort still works from deep in
// a call stack.
func TestADeclaredErrorMayBePanicked(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })
	got := invokeLookup(t, d, r, "raised-by-panic")
	if got.Tag() != witTypes.ResultErr || got.Err().Tag() != types.ToolErrorCustomError ||
		got.Err().CustomError().Name != "offline" {
		t.Fatalf("got %+v", got)
	}
}

// TestMatchRecognisesTheHandlersOwnError — a handler's error is matched the
// same way as a caller's.
func TestMatchRecognisesTheHandlersOwnError(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	_, notFound := declareLookup(r, d)
	err := fmt.Errorf("wrapped: %w", notFound.New(NotFoundPayload{Name: "x"}))
	if p, ok := notFound.Match(err); !ok || p.Name != "x" {
		t.Errorf("Match gave %+v, %v", p, ok)
	}
	if _, ok := notFound.Match(errors.New("other")); ok {
		t.Error("an unrelated error matched")
	}
}

// TestRaisingAnUndeclaredErrorIsAnInvalidResult — the case exists, but this
// command never published it, so the caller was given a contract that does not
// mention it.
func TestRaisingAnUndeclaredErrorIsAnInvalidResult(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })

	got := invokeLookup(t, d, r, "unlisted")
	if got.Tag() != witTypes.ResultErr {
		t.Fatal("raising an undeclared error succeeded")
	}
	if tag := got.Err().Tag(); tag != types.ToolErrorInvalidResult {
		t.Fatalf("error tag %d, want invalid-result", tag)
	}
	if msg := got.Err().InvalidResult(); !strings.Contains(msg, "undeclared error") {
		t.Errorf("message does not explain the problem: %q", msg)
	}
}

// TestAnOrdinaryPanicBecomesAToolError — a crashing handler must not take the
// component down; the agent dispatcher recovers the same way.
func TestAnOrdinaryPanicBecomesAToolError(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })

	got := invokeLookup(t, d, r, "boom")
	if got.Tag() != witTypes.ResultErr {
		t.Fatal("a panicking handler produced a successful invocation")
	}
	if tag := got.Err().Tag(); tag != types.ToolErrorInvalidResult {
		t.Fatalf("error tag %d, want invalid-result", tag)
	}
	if msg := got.Err().InvalidResult(); !strings.Contains(msg, "something went wrong") {
		t.Errorf("message lost the panic: %q", msg)
	}
}

func TestSuccessStillWorksAlongsideDeclaredErrors(t *testing.T) {
	_, r, d := buildToolFor(t, func(r *toolRegistry, d *definitions) { declareLookup(r, d) })

	got := invokeLookup(t, d, r, "ada")
	if got.Tag() != witTypes.ResultOk {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	typed := got.Ok().Result.Some()
	out, err := TypedValue{wit: typed}.JSON()
	if err != nil || out != "found ada" {
		t.Errorf("result %v (%v), want found ada", out, err)
	}
}

func TestDuplicateErrorCaseIsADefinitionError(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	def := defineToolInto[Lookup](r, d, "dupe", ToolSpec{}, false)
	DefineToolError[Unit](def, "same", ToolErrorSpec{})
	DefineToolError[Unit](def, "same", ToolErrorSpec{})
	mustDefErr(t, d, "error case already declared")
}

// TestReflectedCommandErrorsAreDescribed — a caller reading a tool learns its
// declared failures in the SDK's own terms, payload type included.
func TestReflectedCommandErrorsAreDescribed(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	declareLookup(r, d)
	tools, ok := r.discover(d)
	if !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.errs))
	}
	tool := newReflectedTool("lookup", tools[0])
	errs := tool.Root().Errors()
	if len(errs) != 2 {
		t.Fatalf("errors = %+v, want not-found and offline", errs)
	}
	byName := map[string]ReflectedError{}
	for _, e := range errs {
		byName[e.Name] = e
	}
	notFound, offline := byName["not-found"], byName["offline"]
	if notFound.Kind != UsageError || notFound.ExitCode != 2 || notFound.Summary != "no such name" {
		t.Errorf("not-found = %+v", notFound)
	}
	if ref, has := notFound.Payload.Get(); !has {
		t.Error("not-found lost its payload type")
	} else if _, err := ref.PackJSON(map[string]any{"name": "x"}); err != nil {
		t.Errorf("the payload type does not accept its own shape: %v", err)
	}
	if offline.Kind != RuntimeError || offline.ExitCode != 69 || offline.Payload.IsSome() {
		t.Errorf("offline = %+v", offline)
	}

	ctx := &UniversalToolMiddlewareContext[Unit]{inv: &middlewareInvocation{toolName: "lookup", tool: tools[0]}}
	if md := ctx.ToolMetadata(); md.Name() != "lookup" || md.Version() != "1.0.0" {
		t.Errorf("ToolMetadata = %q %q", md.Name(), md.Version())
	}
}
