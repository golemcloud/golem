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
	"strings"
	"testing"

	toolExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_guest"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type NotFoundPayload struct{ Name string }

type LookupArgs struct {
	Name Positional[string]
}

// declareLookup registers a command with two declared failures: one carrying a
// payload and one carrying none.
func declareLookup(r *toolRegistry, d *definitions) (*ToolErrorCase[NotFoundPayload], *ToolErrorCase[Unit], *ToolErrorCase[Unit]) {
	def := defineToolInto(r, d, "lookup", ToolSpec{Version: "1.0.0"})
	notFound := defineToolErrorInto[NotFoundPayload](r, d, def, "not-found", ToolErrorSpec{
		Kind: UsageError, ExitCode: 2, Summary: "no such name",
	})
	offline := defineToolErrorInto[Unit](r, d, def, "offline", ToolErrorSpec{
		Kind: RuntimeError, ExitCode: 69, Summary: "the directory is unreachable",
	})
	unlisted := defineToolErrorInto[Unit](r, d, def, "unlisted", ToolErrorSpec{Kind: RuntimeError})

	cmd := declareCommand[LookupArgs, string](r, d, def, nil, "", LookupArgs{},
		[]CommandOpt{Raises(notFound, offline)})
	handleCommandInto(r, d, cmd, func(_ *ToolContext, in LookupArgs) string {
		switch in.Name.Get() {
		case "missing":
			panic(notFound.New(NotFoundPayload{Name: in.Name.Get()}))
		case "offline":
			panic(offline.New(Unit{}))
		case "unlisted":
			panic(unlisted.New(Unit{}))
		case "boom":
			panic("something went wrong")
		}
		return "found " + in.Name.Get()
	})
	return notFound, offline, unlisted
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

// invokeLookup runs the lookup command with one positional.
func invokeLookup(t *testing.T, d *definitions, r *toolRegistry, name string) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	t.Helper()
	e, _ := r.get("lookup")
	return d.invokeCommand(e, nil, encodeToolArgs(t, d, e, nil, name),
		newToolStdin(toolExports.Stdin{}), newToolStdout(toolExports.Stdout{}))
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
	if name := got.Err().CustomError().Name; name != "offline" {
		t.Errorf("error name %q, want offline", name)
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
	def := defineToolInto(r, d, "dupe", ToolSpec{})
	defineToolErrorInto[Unit](r, d, def, "same", ToolErrorSpec{})
	defineToolErrorInto[Unit](r, d, def, "same", ToolErrorSpec{})
	mustDefErr(t, d, "error case already declared")
}
