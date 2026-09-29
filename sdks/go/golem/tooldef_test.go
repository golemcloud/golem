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
	"io"
	"slices"
	"strings"
	"testing"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The vcs tool below exercises every surface: globals on two levels, a group,
// positionals with defaults, a tail, scalar, optional, list and map options,
// bool and count flags, constraints, standard input and output, declared
// errors and the principal.

type VcsGlobals struct {
	Dir     string
	Verbose uint32
}

type RemoteGlobals struct{ Timeout int32 }

// CommitArgs lists its fields out of canonical order on purpose: the record the
// host sends is ordered by kind, not by the struct.
type CommitArgs struct {
	Amend bool
	VcsGlobals
	Tags    map[string]string
	Message string
	Paths   []string
	Author  Option[string]
	Branch  string
	Include []string
	Signoff bool
	Caller  Principal
}

type CommitResult struct {
	Summary string
	Files   int32
}

type PushArgs struct {
	VcsGlobals
	RemoteGlobals
	Name  string
	Force bool
	In    io.Reader
}

type Rejected struct{ Reason string }

type vcsTool struct {
	tool       *ToolDefinition
	commit     *ToolCommand[CommitArgs, CommitResult]
	push       *ToolStdoutCommand[PushArgs, int32]
	errNothing *ToolErrorCase[Unit]
	errReject  *ToolErrorCase[Rejected]
	seen       *CommitArgs
}

func declareVcs(r *toolRegistry, d *definitions) *vcsTool {
	v := &vcsTool{}
	v.tool = defineToolInto(r, d, "vcs", ToolSpec{Version: "1.2.0", Summary: "A tiny version control tool", RequiresFilesystem: true}, false)
	v.tool.Globals[VcsGlobals](func(g *VcsGlobals, s *ToolGlobalsSpec) {
		s.Option(&g.Dir).Short('C').Default(".").Doc("working directory")
		s.CountFlag(&g.Verbose).Short('v').Max(3)
	})
	v.errNothing = DefineToolError[Unit](v.tool, "nothing-to-commit", ToolErrorSpec{Kind: RuntimeError, ExitCode: 1})
	v.errReject = DefineToolError[Rejected](v.tool, "rejected", ToolErrorSpec{Kind: RuntimeError, ExitCode: 3})

	v.commit = v.tool.Command[CommitArgs, CommitResult]("commit", func(a *CommitArgs, s *ToolCommandSpec) {
		s.Doc("Record changes")
		s.Aliases("ci")
		amend := s.Flag(&a.Amend)
		message := s.Option(&a.Message).Short('m')
		s.Tail(&a.Paths).ValueName("PATH")
		branch := s.Positional(&a.Branch).Default("main")
		author := s.Option(&a.Author).Env("VCS_AUTHOR")
		include := s.List(&a.Include).Delimited(',')
		s.Map(&a.Tags).LastKeyWins()
		s.Flag(&a.Signoff).Negatable().Default(true)
		s.Formatter("text", "plain text")
		s.Formatter("json", "")
		s.DefaultFormatter("json")
		s.Raises(v.errNothing)
		s.Idempotent()
		s.RequiresAny(message, include)
		s.Implies(amend, author)
		s.Forbids(branch.ValueIs("release"), amend, s.Present(&a.Verbose))
		s.Mutex(s.ValueIs(&a.Author, Some("bot")), s.ValueIs(&a.Dir, "/"))
	})
	_ = v.commit.Handle(func(ctx *ToolContext, a CommitArgs) (CommitResult, error) {
		v.seen = &a
		if len(a.Paths) == 0 && !a.Amend {
			return CommitResult{}, v.errNothing.New(Unit{})
		}
		return CommitResult{Summary: ctx.Tool() + " " + strings.Join(ctx.CommandPath(), " ") + ": " + a.Message, Files: int32(len(a.Paths))}, nil
	})

	remote := v.tool.Group("remote").Doc("Manage remotes").Aliases("r")
	remote.Globals[RemoteGlobals](func(g *RemoteGlobals, s *ToolGlobalsSpec) {
		s.Option(&g.Timeout).Default(30)
	})
	v.push = remote.StdoutCommand[PushArgs, int32]("push", func(a *PushArgs, s *ToolCommandSpec) {
		s.Positional(&a.Name)
		s.Flag(&a.Force).Short('f')
		s.Stdin(&a.In).Optional().Mime("text/plain")
		s.StdoutDoc("the pushed bytes, upper-cased")
		s.StdoutMime("text/plain")
		s.Raises(v.errReject)
	})
	_ = v.push.Handle(func(ctx *ToolStdoutContext, a PushArgs) (int32, error) {
		switch a.Name {
		case "forbidden":
			return 0, v.errReject.New(Rejected{Reason: "protected remote"})
		case "undeclared":
			return 0, v.errNothing.New(Unit{})
		case "plain":
			return 0, errors.New("disk full")
		case "panic":
			panic("handler gave up")
		}
		if a.In == nil {
			return 0, nil
		}
		data, err := io.ReadAll(a.In)
		if err != nil {
			return 0, err
		}
		n, err := ctx.Stdout().Write([]byte(strings.ToUpper(string(data))))
		return int32(n) + a.Timeout, err
	})
	return v
}

// readerSource adapts an io.Reader to the stream a host hands a command.
type readerSource struct {
	r    io.Reader
	done bool
}

func (s *readerSource) Read(dst []streamItem) uint32 {
	if s.done {
		return 0
	}
	buf := make([]byte, 3)
	n, err := s.r.Read(buf)
	if err != nil {
		s.done = true
	}
	if n > 0 {
		dst[0] = chunk(string(buf[:n]))
		return 1
	}
	return 0
}

func (s *readerSource) WriterDropped() bool { return s.done }

// loopback routes typed calls to r's dispatcher instead of the host, so a
// native test runs the whole path: defaults, encoding, the host's canonical
// record, decoding and the result. It records the calls it sees.
func loopback(t *testing.T, r *toolRegistry, d *definitions, principal Principal) *[]types.TypedSchemaValue {
	t.Helper()
	var calls []types.TypedSchemaValue
	prev := startToolCall
	t.Cleanup(func() { startToolCall = prev })
	startToolCall = func(tool string, path []string, input types.TypedSchemaValue, stdin io.Reader, stdout bool) (toolCall, error) {
		calls = append(calls, input)
		e, ok := r.get(tool)
		if !ok {
			return toolCall{}, toolCallErrorFromWit(tool, path, types.MakeToolRpcErrorNotFound(tool))
		}
		in := &ToolStdin{absent: absentStdin}
		if stdin != nil {
			in = &ToolStdin{src: &readerSource{r: stdin}}
		}
		sink := &fakeSink{}
		out := &ToolStdout{absent: absentStdout}
		if stdout {
			out = &ToolStdout{sink: sink}
		}
		res := d.invokeCommand(e, path, input, in, out, principal)

		var reader *ToolStdin
		if stdout {
			items := []streamItem{}
			if len(sink.written) > 0 {
				items = append(items, chunk(string(sink.written)))
			}
			if sink.failed != nil {
				items = append(items, failure(*sink.failed))
			}
			reader = &ToolStdin{src: &fakeSource{items: items}}
		}
		return toolCall{
			stdout: reader,
			wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
				if res.Tag() == witTypes.ResultErr {
					e := types.MakeToolRpcErrorRemoteToolError(res.Err())
					return witTypes.None[types.TypedSchemaValue](), &e
				}
				return res.Ok().Result, nil
			},
			cancel: func() {},
		}, nil
	}
	return &calls
}

func newVcs(t *testing.T) (*vcsTool, *toolRegistry, *definitions) {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	v := declareVcs(r, d)
	if _, ok := r.discover(d); !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.errs))
	}
	return v, r, d
}

func names[T any](items []T, name func(T) string) []string {
	out := make([]string, 0, len(items))
	for _, it := range items {
		out = append(out, name(it))
	}
	return out
}

func optionNames(os []toolCommon.OptionSpec) []string {
	return names(os, func(o toolCommon.OptionSpec) string { return o.Long })
}

func flagNames(fs []toolCommon.FlagSpec) []string {
	return names(fs, func(f toolCommon.FlagSpec) string { return f.Long })
}

func TestToolMetadataDescribesTheCommandTree(t *testing.T) {
	_, r, d := newVcs(t)
	e, _ := r.get("vcs")
	tool, _ := d.buildTool(e)
	nodes := tool.Commands.Nodes

	if !tool.RequiresFilesystem {
		t.Error("the tool lost RequiresFilesystem")
	}
	root := nodes[0]
	if root.Name != "vcs" || root.Doc.Summary != "A tiny version control tool" || root.Body.IsSome() {
		t.Errorf("root node: %+v", root)
	}
	if got := optionNames(root.Globals.Options); !slices.Equal(got, []string{"dir"}) {
		t.Errorf("root global options %v", got)
	}
	if got := flagNames(root.Globals.Flags); !slices.Equal(got, []string{"verbose"}) {
		t.Errorf("root global flags %v", got)
	}
	if verbose := root.Globals.Flags[0].Shape; verbose.Tag() != toolCommon.FlagShapeCountFlag || verbose.CountFlag().Some() != 3 {
		t.Errorf("verbose is not a count flag capped at 3")
	}
	if dir := root.Globals.Options[0]; dir.Required || dir.Default.IsNone() || dir.Short.Some() != 'C' {
		t.Errorf("dir: %+v", dir)
	}
	subs := names(root.Subcommands, func(i int32) string { return nodes[i].Name })
	if !slices.Equal(subs, []string{"commit", "remote"}) {
		t.Fatalf("subcommands %v", subs)
	}

	commit := nodes[root.Subcommands[0]]
	if commit.Doc.Summary != "Record changes" || !slices.Equal(commit.Aliases, []string{"ci"}) {
		t.Errorf("commit node: %+v", commit)
	}
	body := commit.Body.Some()
	fixed := body.Positionals.Fixed
	if len(fixed) != 1 || fixed[0].Name != "branch" || fixed[0].Required || fixed[0].Default.IsNone() {
		t.Errorf("positionals: %+v", fixed)
	}
	if tail := body.Positionals.Tail; tail.IsNone() || tail.Some().Name != "paths" || tail.Some().ValueName.Some() != "PATH" {
		t.Errorf("tail: %+v", tail)
	}
	if got := optionNames(body.Options); !slices.Equal(got, []string{"message", "author", "include", "tags"}) {
		t.Errorf("options %v", got)
	}
	message, author, include, tags := body.Options[0], body.Options[1], body.Options[2], body.Options[3]
	if !message.Required || message.Shape.Tag() != toolCommon.OptionShapeScalar {
		t.Errorf("message: %+v", message)
	}
	if author.Required || author.EnvVar.Some() != "VCS_AUTHOR" || author.Default.IsSome() {
		t.Errorf("author: %+v", author)
	}
	if include.Shape.Tag() != toolCommon.OptionShapeRepeatableList ||
		include.Shape.RepeatableList().Repetition.Tag() != toolCommon.RepetitionDelimited {
		t.Errorf("include is not a delimited list")
	}
	if tags.Shape.Tag() != toolCommon.OptionShapeRepeatableMap ||
		tags.Shape.RepeatableMap().DuplicateKeyPolicy != toolCommon.DuplicateKeyPolicyLastWins {
		t.Errorf("tags is not a last-wins map")
	}
	if got := flagNames(body.Flags); !slices.Equal(got, []string{"amend", "signoff"}) {
		t.Errorf("flags %v", got)
	}
	if signoff := body.Flags[1].Shape.BoolFlag(); !signoff.Default || !signoff.Negatable {
		t.Errorf("signoff: %+v", signoff)
	}
	res := body.Result.Some()
	if got := names(res.Formatters, func(f toolCommon.Formatter) string { return f.Name }); !slices.Equal(got, []string{"text", "json"}) ||
		res.DefaultFormatter != "json" {
		t.Errorf("formatters %v default %s", got, res.DefaultFormatter)
	}
	if body.Annotations.IsNone() || !body.Annotations.Some().Idempotent {
		t.Error("commit is not annotated idempotent")
	}
	if len(body.Errors) != 1 || body.Errors[0].Name != "nothing-to-commit" || body.Errors[0].Payload.IsSome() {
		t.Errorf("errors: %+v", body.Errors)
	}

	cs := body.Constraints
	if len(cs) != 4 {
		t.Fatalf("%d constraints, want 4", len(cs))
	}
	if cs[0].Tag() != toolCommon.ConstraintRequiresAny || refNames(cs[0].RequiresAny()) != "message include" {
		t.Errorf("requires-any: %s", refNames(cs[0].RequiresAny()))
	}
	if imp := cs[1].Implies(); refNames(imp.Lhs) != "amend" || refNames(imp.Rhs) != "author" {
		t.Errorf("implies: %s -> %s", refNames(imp.Lhs), refNames(imp.Rhs))
	}
	if fb := cs[2].Forbids(); refNames(fb.Lhs) != "branch=" || refNames(fb.Rhs) != "amend verbose" {
		t.Errorf("forbids: %s / %s", refNames(fb.Lhs), refNames(fb.Rhs))
	}
	groups := cs[3].MutexGroups()
	if len(groups) != 2 || refNames(groups[0].Refs) != "author=" || refNames(groups[1].Refs) != "dir=" {
		t.Errorf("mutex groups: %+v", groups)
	}

	remote := nodes[root.Subcommands[1]]
	if remote.Body.IsSome() || !slices.Equal(remote.Aliases, []string{"r"}) ||
		!slices.Equal(optionNames(remote.Globals.Options), []string{"timeout"}) {
		t.Errorf("remote node: %+v", remote)
	}
	push := nodes[remote.Subcommands[0]].Body.Some()
	if push.Stdout.IsNone() || push.Stdout.Some().Doc.Summary != "the pushed bytes, upper-cased" {
		t.Errorf("push stdout: %+v", push.Stdout)
	}
	if push.Stdin.IsNone() || push.Stdin.Some().Required || !slices.Equal(push.Stdin.Some().Mime, []string{"text/plain"}) {
		t.Errorf("push stdin: %+v", push.Stdin)
	}
}

func refNames(refs []toolCommon.Ref) string {
	out := make([]string, 0, len(refs))
	for _, r := range refs {
		if r.Tag() == toolCommon.RefPresent {
			out = append(out, r.Present())
		} else {
			out = append(out, r.ValueIs().Name+"=")
		}
	}
	return strings.Join(out, " ")
}

// TestToolCallRoundTripsInCanonicalOrder — the struct's field order differs
// from the canonical record's, which is the order the host sends; defaults
// travel for what fill leaves alone.
func TestToolCallRoundTripsInCanonicalOrder(t *testing.T) {
	v, r, d := newVcs(t)
	loopback(t, r, d, AgentPrincipal{AgentID: AgentID{AgentID: "caller()"}})

	res, err := v.commit.Call(func(a *CommitArgs) {
		a.Message = "fix the build"
		a.Paths = []string{"a.go", "b.go"}
		a.Tags = map[string]string{"k": "v"}
		a.Include = []string{"x"}
		a.Verbose = 2
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.Summary != "vcs commit: fix the build" || res.Files != 2 {
		t.Errorf("result %+v", res)
	}
	seen := v.seen
	if seen.Branch != "main" || seen.Dir != "." || !seen.Signoff || seen.Amend || seen.Verbose != 2 {
		t.Errorf("defaults did not arrive: %+v", seen)
	}
	if !slices.Equal(seen.Paths, []string{"a.go", "b.go"}) || seen.Tags["k"] != "v" ||
		!slices.Equal(seen.Include, []string{"x"}) || seen.Author.IsSome() {
		t.Errorf("arguments did not arrive: %+v", seen)
	}
	if p, ok := seen.Caller.(AgentPrincipal); !ok || p.AgentID.AgentID != "caller()" {
		t.Errorf("principal %+v", seen.Caller)
	}
}

// TestToolCallInputIsTheCanonicalRecord — the input the SDK sends is a record
// whose fields are exactly the canonical input model reflection derives from
// the published metadata, so the host accepts it; and reflection's own packing
// decodes the same way.
func TestToolCallInputIsTheCanonicalRecord(t *testing.T) {
	v, r, d := newVcs(t)
	calls := loopback(t, r, d, AnonymousPrincipal{})
	if _, err := v.commit.Call(func(a *CommitArgs) { a.Message = "m"; a.Amend = true }); err != nil {
		t.Fatal(err)
	}
	sent := (*calls)[0]
	rootBody := sent.Graph.TypeNodes[sent.Graph.Root].Body
	if rootBody.Tag() != types.SchemaTypeBodyRecordType {
		t.Fatalf("input graph root is not a record")
	}
	sentNames := names(rootBody.RecordType(), func(f types.NamedFieldType) string { return f.Name })
	want := []string{"dir", "verbose", "branch", "paths", "message", "author", "include", "tags", "amend", "signoff"}
	if !slices.Equal(sentNames, want) {
		t.Errorf("sent fields %v, want %v", sentNames, want)
	}

	e, _ := r.get("vcs")
	tool, _ := d.buildTool(e)
	cmd, ok := newReflectedTool("vcs", tool).Command([]string{"commit"})
	if !ok {
		t.Fatal("reflection does not find commit")
	}
	params, err := cmd.Arguments()
	if err != nil {
		t.Fatal(err)
	}
	if got := names(params, func(p core.Parameter) string { return p.Name }); !slices.Equal(got, want) {
		t.Errorf("reflected fields %v, want %v", got, want)
	}

	packed, err := cmd.PackJSON(map[string]any{
		"dir": "/src", "verbose": 1, "branch": "dev", "paths": []any{"z"}, "message": "via reflection",
		"author": "ann", "include": []any{}, "tags": []any{}, "amend": false, "signoff": false,
	})
	if err != nil {
		t.Fatal(err)
	}
	if root := packed.wit.Graph.TypeNodes[packed.wit.Graph.Root].Body; root.Tag() != types.SchemaTypeBodyRecordType {
		t.Fatal("reflection's input graph root is not a record")
	}
	if got := d.invokeCommand(e, []string{"ci"}, packed.wit, nil, &ToolStdout{absent: absentStdout}, nil); got.IsErr() {
		t.Fatalf("invoke failed: %+v", got.Err())
	}
	if s := v.seen; s.Dir != "/src" || s.Branch != "dev" || s.Author.Unwrap() != "ann" || s.Signoff || s.Verbose != 1 {
		t.Errorf("reflection-packed arguments decoded as %+v", s)
	}
}

func TestToolCallReportsDeclaredErrors(t *testing.T) {
	v, r, d := newVcs(t)
	loopback(t, r, d, AnonymousPrincipal{})

	_, err := v.commit.Call(func(a *CommitArgs) { a.Message = "nothing" })
	var ce *ToolCallError
	if !errors.As(err, &ce) || ce.Kind != ToolCallDeclaredError || ce.ErrorName != "nothing-to-commit" {
		t.Fatalf("got %v", err)
	}
	if _, ok := v.errNothing.Match(err); !ok {
		t.Error("the case does not match its own error")
	}
	if _, ok := v.errReject.Match(err); ok {
		t.Error("another case matches")
	}

	inv, err := v.push.Call(func(a *PushArgs) { a.Name = "forbidden" })
	if err != nil {
		t.Fatal(err)
	}
	_, err = inv.Wait()
	if rej, ok := v.errReject.Match(err); !ok || rej.Reason != "protected remote" {
		t.Errorf("match gave %+v, %v for %v", rej, ok, err)
	}
	if !strings.Contains(err.Error(), "remote push") {
		t.Errorf("error does not name the command: %v", err)
	}
}

func TestToolHandlerFailuresAreInvalidResults(t *testing.T) {
	v, r, d := newVcs(t)
	loopback(t, r, d, AnonymousPrincipal{})
	for name, want := range map[string]string{
		"undeclared": `undeclared error "nothing-to-commit"`,
		"plain":      "command remote push failed: disk full",
		"panic":      "command remote push panicked: handler gave up",
	} {
		inv, err := v.push.Call(func(a *PushArgs) { a.Name = name })
		if err != nil {
			t.Fatal(err)
		}
		_, err = inv.Wait()
		var ce *ToolCallError
		if !errors.As(err, &ce) || ce.Kind != ToolCallInvalidResult || !strings.Contains(ce.Message, want) {
			t.Errorf("%s: got %v, want an invalid result mentioning %q", name, err, want)
		}
	}
}

func TestStdoutCommandStreamsItsOutput(t *testing.T) {
	v, r, d := newVcs(t)
	loopback(t, r, d, AnonymousPrincipal{})

	inv, err := v.push.Call(func(a *PushArgs) {
		a.Name = "origin"
		a.In = strings.NewReader("hello world")
	})
	if err != nil {
		t.Fatal(err)
	}
	out, n, err := inv.Collect()
	if err != nil {
		t.Fatal(err)
	}
	if string(out) != "HELLO WORLD" || n != 11+30 {
		t.Errorf("got %q and %d", out, n)
	}

	inv, _ = v.push.Call(func(a *PushArgs) { a.Name = "panic" })
	_, err = io.ReadAll(inv.Stdout())
	var se *StreamError
	if !errors.As(err, &se) || se.Failure.String() != "failed: command remote push panicked: handler gave up" {
		t.Errorf("stdout of a panicking command ended with %v", err)
	}
}

type CatArgs struct{ In io.Reader }

func TestRequiredStdinIsRefusedBeforeSending(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	tool := defineToolInto(r, d, "cat", ToolSpec{}, false)
	cat := tool.Body[CatArgs, string](func(a *CatArgs, s *ToolCommandSpec) { s.Stdin(&a.In) })
	_ = cat.Handle(func(_ *ToolContext, a CatArgs) (string, error) {
		data, err := io.ReadAll(a.In)
		return string(data), err
	})
	calls := loopback(t, r, d, AnonymousPrincipal{})

	_, err := cat.Call(nil)
	var ce *ToolCallError
	if !errors.As(err, &ce) || ce.Kind != ToolCallInvalidInput || len(*calls) != 0 {
		t.Errorf("got %v after %d calls", err, len(*calls))
	}
	got, err := cat.Call(func(a *CatArgs) { a.In = strings.NewReader("meow") })
	if err != nil || got != "meow" {
		t.Errorf("got %q, %v", got, err)
	}

	e, _ := r.get("cat")
	res := d.invokeCommand(e, nil, (*calls)[0], &ToolStdin{absent: absentStdin}, &ToolStdout{absent: absentStdout}, nil)
	if res.IsOk() || res.Err().Tag() != types.ToolErrorInvalidInput {
		t.Errorf("a host invocation without the required stdin was accepted")
	}
}

func TestToolInvocationRejectsUnknownCommandsAndMalformedInput(t *testing.T) {
	_, r, d := newVcs(t)
	e, _ := r.get("vcs")
	none := &ToolStdout{absent: absentStdout}

	res := d.invokeCommand(e, []string{"nope"}, types.TypedSchemaValue{}, nil, none, nil)
	if res.IsOk() || res.Err().Tag() != types.ToolErrorInvalidCommandPath {
		t.Error("an unknown command was accepted")
	}
	res = d.invokeCommand(e, []string{"remote"}, types.TypedSchemaValue{}, nil, none, nil)
	if res.IsOk() || res.Err().Tag() != types.ToolErrorInvalidCommandPath {
		t.Error("a group without a body was invoked")
	}
	short, _ := EncodeTypedValue(struct{ Message string }{"m"})
	res = d.invokeCommand(e, []string{"commit"}, short.wit, nil, none, nil)
	if res.IsOk() || res.Err().Tag() != types.ToolErrorInvalidInput {
		t.Error("a record with the wrong field count was accepted")
	}
}

type BadArgs struct {
	Name   string
	Other  string
	Opt    Option[string]
	Pos    Option[string]
	Req    string
	Tail1  []string
	Tail2  []string
	Global VcsGlobals
}

type SingleArgs struct{ Name string }

type EmbedArgs struct {
	VcsGlobals
	Name string
}

func TestToolDeclarationErrors(t *testing.T) {
	cases := map[string]struct {
		declare func(tool *ToolDefinition)
		want    string
	}{
		"unbound field": {func(tool *ToolDefinition) {
			c := tool.Command[SingleArgs, string]("x", nil)
			_ = c.Handle(func(*ToolContext, SingleArgs) (string, error) { return "", nil })
		}, "field Name is not bound"},
		"bound twice": {func(tool *ToolDefinition) {
			tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) {
				s.Option(&a.Name)
				s.Positional(&a.Name)
			})
		}, "field Name is bound twice"},
		"foreign pointer": {func(tool *ToolDefinition) {
			var elsewhere string
			tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) {
				s.Option(&elsewhere)
			})
		}, "does not address a field"},
		"optional with default": {func(tool *ToolDefinition) {
			c := tool.Command[BadArgs, string]("x", func(a *BadArgs, s *ToolCommandSpec) {
				s.Option(&a.Opt).Default(Some("x"))
			})
			_ = c.Handle(func(*ToolContext, BadArgs) (string, error) { return "", nil })
		}, "opt is optional (golem.Option) and cannot also have a default"},
		"required after optional": {func(tool *ToolDefinition) {
			c := tool.Command[BadArgs, string]("x", func(a *BadArgs, s *ToolCommandSpec) {
				s.Positional(&a.Pos)
				s.Positional(&a.Req)
			})
			_ = c.Handle(func(*ToolContext, BadArgs) (string, error) { return "", nil })
		}, "the required positional req follows the optional positional pos"},
		"two tails": {func(tool *ToolDefinition) {
			c := tool.Command[BadArgs, string]("x", func(a *BadArgs, s *ToolCommandSpec) {
				s.Tail(&a.Tail1)
				s.Tail(&a.Tail2)
			})
			_ = c.Handle(func(*ToolContext, BadArgs) (string, error) { return "", nil })
		}, "binds 2 tails"},
		"missing globals embedding": {func(tool *ToolDefinition) {
			tool.Globals[VcsGlobals](func(g *VcsGlobals, s *ToolGlobalsSpec) { s.Option(&g.Dir) })
			c := tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) { s.Option(&a.Name) })
			_ = c.Handle(func(*ToolContext, SingleArgs) (string, error) { return "", nil })
		}, "must embed golem.VcsGlobals"},
		"global bound by the command": {func(tool *ToolDefinition) {
			tool.Globals[VcsGlobals](func(g *VcsGlobals, s *ToolGlobalsSpec) { s.Option(&g.Dir) })
			c := tool.Command[EmbedArgs, string]("x", func(a *EmbedArgs, s *ToolCommandSpec) {
				s.Option(&a.Name)
				s.Option(&a.Dir)
			})
			_ = c.Handle(func(*ToolContext, EmbedArgs) (string, error) { return "", nil })
		}, "field Dir belongs to the embedded globals"},
		"formatters on a unit result": {func(tool *ToolDefinition) {
			c := tool.Command[SingleArgs, Unit]("x", func(a *SingleArgs, s *ToolCommandSpec) {
				s.Option(&a.Name)
				s.Formatters("json")
			})
			_ = c.Handle(func(*ToolContext, SingleArgs) (Unit, error) { return Unit{}, nil })
		}, "declares formatters but returns no result"},
		"undeclared default formatter": {func(tool *ToolDefinition) {
			c := tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) {
				s.Option(&a.Name)
				s.Formatters("json")
				s.DefaultFormatter("yaml")
			})
			_ = c.Handle(func(*ToolContext, SingleArgs) (string, error) { return "", nil })
		}, `defaults to the formatter "yaml"`},
		"stdout settings on a plain command": {func(tool *ToolDefinition) {
			tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) {
				s.Option(&a.Name)
				s.StdoutMime("text/plain")
			})
		}, "declare it with StdoutCommand"},
		"value-is on a flag": {func(tool *ToolDefinition) {
			c := tool.Command[CommitArgs, string]("x", func(a *CommitArgs, s *ToolCommandSpec) {
				s.Option(&a.Message)
				s.Option(&a.Author)
				s.Positional(&a.Branch)
				s.Tail(&a.Paths)
				s.List(&a.Include)
				s.Map(&a.Tags)
				s.Flag(&a.Signoff)
				s.RequiresAll(s.ValueIs(&a.Amend, true))
				s.Flag(&a.Amend)
			})
			_ = c.Handle(func(*ToolContext, CommitArgs) (string, error) { return "", nil })
		}, "compares the flag amend with a value"},
		"no handler": {func(tool *ToolDefinition) {
			tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) { s.Option(&a.Name) })
		}, "command x has no handler"},
		"duplicate command": {func(tool *ToolDefinition) {
			tool.Group("x")
			tool.Group("x")
		}, "command already declared: x"},
		"foreign error case": {func(tool *ToolDefinition) {
			other := defineToolInto(tool.entry.r, tool.entry.d, "other", ToolSpec{}, false)
			errOther := DefineToolError[Unit](other, "boom", ToolErrorSpec{})
			c := tool.Command[SingleArgs, string]("x", func(a *SingleArgs, s *ToolCommandSpec) {
				s.Option(&a.Name)
				s.Raises(errOther)
			})
			_ = c.Handle(func(*ToolContext, SingleArgs) (string, error) { return "", nil })
		}, "raises boom, an error declared on the tool other"},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			r, d := newToolRegistry(), newDefinitions()
			tool := defineToolInto(r, d, "t", ToolSpec{}, false)
			tc.declare(tool)
			if _, ok := r.discover(d); ok && len(d.errs) == 0 {
				t.Fatalf("no definition error, want %q", tc.want)
			}
			if msg := allDefErrors(d.errs); !strings.Contains(msg, tc.want) {
				t.Errorf("errors:\n%s\nwant one containing %q", msg, tc.want)
			}
		})
	}
}

func TestRemoteToolsAreDeclaredForCallingOnly(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	remote := defineToolInto(r, d, "elsewhere", ToolSpec{}, true)
	cmd := remote.Command[SingleArgs, string]("run", func(a *SingleArgs, s *ToolCommandSpec) { s.Positional(&a.Name) })
	if tools, ok := r.discover(d); !ok || len(tools) != 0 {
		t.Errorf("a remote tool was exported: %d tools", len(tools))
	}
	if _, ok := d.buildTool(remote.entry); !ok {
		t.Errorf("a remote tool without handlers is not well-defined: %s", allDefErrors(d.errs))
	}
	_ = cmd.Handle(func(*ToolContext, SingleArgs) (string, error) { return "", nil })
	if msg := allDefErrors(d.errs); !strings.Contains(msg, "belongs to a remote tool") {
		t.Errorf("handling a remote command was accepted: %s", msg)
	}
}

func TestKebabNames(t *testing.T) {
	for in, want := range map[string]string{
		"Name": "name", "GitDir": "git-dir", "URLPath": "url-path", "MaxCount": "max-count",
		"HTTP": "http", "Retry2Times": "retry2-times", "ID": "id",
	} {
		if got := kebab(in); got != want {
			t.Errorf("kebab(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestToolCallOutsideAComponentSaysSo(t *testing.T) {
	v, _, _ := newVcs(t)
	_, err := v.commit.Call(func(a *CommitArgs) { a.Message = "m" })
	if err == nil || !strings.Contains(err.Error(), "only available inside a component") {
		t.Errorf("got %v", err)
	}
}
