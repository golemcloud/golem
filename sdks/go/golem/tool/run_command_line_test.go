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

package tool_test

import (
	"bytes"
	"io"
	"slices"
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/reflection"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type ProcFixture struct{}

type ProcStatusArgs struct {
	In io.Reader
}

type ProcNameArgs struct{}

type ProcReport struct {
	Name string
	Size uint32
}

// procTool declares commands with each result shape a process projection
// distinguishes: an exit status with stdin and both outputs, a structured
// result, a text result, a result beside a declared stdout, and a declared
// error with its own status.
func procTool(t *testing.T) *reflection.ToolClient {
	t.Helper()
	meta := tool.DeclaredMetadata(t, "proc", tool.Spec{Version: "1.0.0", Summary: "Process shapes"}, nil,
		func(d *tool.Definition[ProcFixture]) {
			d.OutputCommand[ProcStatusArgs, golem.ProcessExitStatus]("status", func(a *ProcStatusArgs, s *tool.CommandSpec) {
				s.Doc("Report a status")
				s.Stdin(&a.In).Optional()
				s.Stdout()
				s.Stderr()
			}).Handle(func(*tool.OutputContext, ProcStatusArgs) (golem.ProcessExitStatus, error) {
				return golem.ProcessExitStatus{}, nil
			})
			d.Command[ProcNameArgs, ProcReport]("describe", func(*ProcNameArgs, *tool.CommandSpec) {}).
				Handle(func(*tool.Context, ProcNameArgs) (ProcReport, error) { return ProcReport{}, nil })
			d.Command[ProcNameArgs, string]("name", func(*ProcNameArgs, *tool.CommandSpec) {}).
				Handle(func(*tool.Context, ProcNameArgs) (string, error) { return "", nil })
			d.OutputCommand[ProcNameArgs, string]("write", func(_ *ProcNameArgs, s *tool.CommandSpec) { s.Stdout() }).
				Handle(func(*tool.OutputContext, ProcNameArgs) (string, error) { return "", nil })
			locked := tool.DefineToolError[string](d, "locked", tool.ErrorSpec{Kind: tool.RuntimeError, ExitCode: 4, Summary: "Locked"})
			d.Command[ProcNameArgs, string]("fail", func(_ *ProcNameArgs, s *tool.CommandSpec) { s.Raises(locked) }).
				Handle(func(*tool.Context, ProcNameArgs) (string, error) { return "", nil })
		})
	return reflection.ToolOf(meta).Bind()
}

func customError[P any](name string, payload P) *types.ToolRpcError {
	e := types.MakeToolRpcErrorRemoteToolError(types.MakeToolErrorCustomError(types.CustomToolError{
		Name: name, Payload: witOf(golem.EncodeTypedValue(payload)),
	}))
	return &e
}

func rpcError(e types.ToolRpcError) *types.ToolRpcError { return &e }

func result[T any](v T) witTypes.Option[types.TypedSchemaValue] {
	return witTypes.Some(witOf(golem.EncodeTypedValue(v)))
}

// reshapeStatus returns a ReservedStatusMetadata rewrite that gives the
// exit_code field the type body, renames it to rename, or adds the field extra,
// for each of them that is set.
func reshapeStatus(body *types.SchemaTypeBody, extra *types.NamedFieldType, rename string) func(*types.SchemaGraph, []types.NamedFieldType) []types.NamedFieldType {
	return func(g *types.SchemaGraph, fields []types.NamedFieldType) []types.NamedFieldType {
		if body != nil {
			g.TypeNodes = append(g.TypeNodes, types.SchemaTypeNode{Body: *body})
			fields[0].Body = int32(len(g.TypeNodes) - 1)
		}
		if rename != "" {
			fields[0].Name = rename
		}
		if extra != nil {
			g.TypeNodes = append(g.TypeNodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyStringType()})
			extra.Body = int32(len(g.TypeNodes) - 1)
			fields = append(fields, *extra)
		}
		return fields
	}
}

// A command that names the reserved ProcessExitStatus type with any shape other
// than exactly one exit_code: u8 field is refused before it runs, as the Rust
// process adapter shared by bash and JavaScript refuses it.
func TestRunCommandLineRefusesAMalformedProcessExitStatus(t *testing.T) {
	text := types.MakeSchemaTypeBodyStringType()
	wide := types.MakeSchemaTypeBodyU16Type(witTypes.None[types.NumericRestrictions]())
	restricted := types.MakeSchemaTypeBodyU8Type(witTypes.Some(types.NumericRestrictions{}))
	cases := []struct {
		name  string
		meta  tool.Metadata
		valid bool
	}{
		{"exit_code as string", tool.ReservedStatusMetadata(t, reshapeStatus(&text, nil, "")), false},
		{"exit_code as u16", tool.ReservedStatusMetadata(t, reshapeStatus(&wide, nil, "")), false},
		{"restricted u8", tool.ReservedStatusMetadata(t, reshapeStatus(&restricted, nil, "")), false},
		{"an extra field", tool.ReservedStatusMetadata(t, reshapeStatus(nil, &types.NamedFieldType{Name: "note"}, "")), false},
		{"another field name", tool.ReservedStatusMetadata(t, reshapeStatus(nil, nil, "code")), false},
		{"the shared shape", tool.ReservedStatusMetadata(t, reshapeStatus(nil, nil, "")), true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			calls := 0
			tool.ScriptToolCalls(t, func([]string, io.Reader) tool.ScriptedCall {
				calls++
				return tool.ScriptedCall{Result: result(golem.ProcessExitStatus{ExitCode: 7})}
			})
			var stdout, stderr bytes.Buffer
			code := reflection.ToolOf(c.meta).Bind().RunCommandLine([]string{"run"}, nil, nil, &stdout, &stderr)
			want := struct {
				code   uint8
				stderr string
				calls  int
			}{2, "ProcessExitStatus must define exactly one exit_code: u8 field\n", 0}
			if c.valid {
				want.code, want.stderr, want.calls = 7, "", 1
			}
			if code != want.code || stdout.String() != "" || stderr.String() != want.stderr || calls != want.calls {
				t.Errorf("got %d, stdout %q, stderr %q, %d call(s); want %d, \"\", %q, %d call(s)",
					code, stdout.String(), stderr.String(), calls, want.code, want.stderr, want.calls)
			}
		})
	}
}

func TestRunCommandLineProjectsACallAsAProcess(t *testing.T) {
	client := procTool(t)
	cases := []struct {
		name           string
		args           []string
		stdin          string
		call           tool.ScriptedCall
		wantStdin      string // "" means the call must carry no stdin
		code           uint8
		stdout, stderr string
		called         bool
	}{
		{name: "help", args: []string{"status", "--help"}, code: 0,
			stdout: "Usage: proc status\n\nReport a status\n\nOptions:\n  -h, --help      Print help (reserved only when undeclared)\n"},
		{name: "usage error", args: []string{"status", "-z"}, code: 2, stderr: "unknown short option -z\n"},
		{name: "status with stdin and outputs", args: []string{"status"}, stdin: "input",
			call:      tool.ScriptedCall{Stdout: []byte("out"), Stderr: []byte("err"), Result: result(golem.ProcessExitStatus{ExitCode: 7})},
			wantStdin: "input", code: 7, stdout: "out", stderr: "err", called: true},
		{name: "structured result as JSON, stdin withheld", args: []string{"describe"}, stdin: "unused",
			call: tool.ScriptedCall{Result: result(ProcReport{Name: "a<b", Size: 3})},
			code: 0, stdout: "{\"name\":\"a<b\",\"size\":3}\n", called: true},
		{name: "text result as it is", args: []string{"name"},
			call: tool.ScriptedCall{Result: result("hello")}, code: 0, stdout: "hello", called: true},
		{name: "declared stdout wins over the result", args: []string{"write"},
			call: tool.ScriptedCall{Stdout: []byte("streamed"), Result: result("ignored")}, code: 0, stdout: "streamed", called: true},
		{name: "declared error status", args: []string{"fail"},
			call: tool.ScriptedCall{Err: customError("locked", "busy")}, code: 4, stderr: "tool error: locked: \"busy\"\n", called: true},
		{name: "undeclared error status", args: []string{"fail"},
			call: tool.ScriptedCall{Err: customError("other", "x")}, code: 1, stderr: "tool error: other: \"x\"\n", called: true},
		{name: "denied", args: []string{"name"},
			call: tool.ScriptedCall{Err: rpcError(types.MakeToolRpcErrorDenied("no grant"))}, code: 3, stderr: "no grant\n", called: true},
		{name: "not found", args: []string{"name"},
			call: tool.ScriptedCall{Err: rpcError(types.MakeToolRpcErrorNotFound("gone"))}, code: 127, stderr: "gone\n", called: true},
		{name: "cancelled", args: []string{"name"},
			call: tool.ScriptedCall{Err: rpcError(types.MakeToolRpcErrorCancelled())}, code: 130, stderr: "Cancelled\n", called: true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			var calls [][]string
			var gotStdin *string
			tool.ScriptToolCalls(t, func(path []string, stdin io.Reader) tool.ScriptedCall {
				calls = append(calls, slices.Clone(path))
				if stdin != nil {
					data, _ := io.ReadAll(stdin)
					s := string(data)
					gotStdin = &s
				}
				return c.call
			})
			var stdin io.Reader
			if c.stdin != "" {
				stdin = strings.NewReader(c.stdin)
			}
			var stdout, stderr bytes.Buffer
			code := client.RunCommandLine(c.args, nil, stdin, &stdout, &stderr)
			if code != c.code || stdout.String() != c.stdout || stderr.String() != c.stderr {
				t.Errorf("got %d, stdout %q, stderr %q; want %d, %q, %q",
					code, stdout.String(), stderr.String(), c.code, c.stdout, c.stderr)
			}
			if c.called != (len(calls) == 1) || len(calls) > 1 {
				t.Errorf("calls %v, want called=%v", calls, c.called)
			}
			switch {
			case c.wantStdin == "" && gotStdin != nil:
				t.Errorf("the call carried stdin %q, want none", *gotStdin)
			case c.wantStdin != "" && (gotStdin == nil || *gotStdin != c.wantStdin):
				t.Errorf("the call carried stdin %v, want %q", gotStdin, c.wantStdin)
			}
		})
	}
}
