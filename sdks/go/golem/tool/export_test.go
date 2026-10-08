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
	"io"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Hooks for the external tests in this directory, which exercise the
// reflection package against tools declared here.

// FilesMetadata declares the files fixture in a fresh registry and returns its
// published metadata.
func FilesMetadata(t *testing.T) Metadata {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	def := defineToolInto[Files](r, d, "files", Spec{Version: "1.0.0", Summary: "File utilities"}, false)
	index := def.Group("index").Doc("Manage the index")

	type AddArgs struct {
		Path    string
		Force   bool
		Retries int32
	}
	add := index.Command[AddArgs, string]("add", func(a *AddArgs, s *CommandSpec) {
		s.Doc("Add a file")
		s.Aliases("a")
		s.Positional(&a.Path)
		s.Flag(&a.Force).Short('f')
		s.Option(&a.Retries).Default(1)
	})
	_ = add.Handle(func(_ *Context, in AddArgs) (string, error) { return in.Path, nil })
	return metadataOf(t, r, d, "files")
}

// Files is the files fixture's identity type.
type Files struct{}

// LookupMetadata declares the lookup fixture in a fresh registry and returns
// its published metadata.
func LookupMetadata(t *testing.T) Metadata {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	declareLookup(r, d)
	return metadataOf(t, r, d, "lookup")
}

// VcsFixture declares the vcs fixture in a fresh registry. It returns the
// published metadata, a function that invokes the ci command with an input as
// the host would deliver it, and the arguments the handler last saw.
func VcsFixture(t *testing.T) (Metadata, func(types.TypedSchemaValue) error, func() CommitArgs) {
	t.Helper()
	v, r, d := newVcs(t)
	invoke := func(input types.TypedSchemaValue) error {
		e, _ := r.get("vcs")
		if got := d.invokeCommand(e, []string{"ci"}, input, nil, hostOutputs{}, nil); got.IsErr() {
			return &CallError{Kind: CallInvalidInput, Message: "invoke failed"}
		}
		return nil
	}
	return metadataOf(t, r, d, "vcs"), invoke, func() CommitArgs { return *v.seen }
}

func metadataOf(t *testing.T, r *toolRegistry, d *definitions, name string) Metadata {
	t.Helper()
	tools, ok := r.discover(d)
	if !ok {
		t.Fatalf("tool discovery failed: %s", engine.AllErrors(d.Errs))
	}
	for i, n := range r.order {
		if n == name {
			return Metadata{name: name, wit: tools[i]}
		}
	}
	t.Fatalf("no tool %s", name)
	return Metadata{}
}

// RecordToolCalls routes tool calls to a scripted outcome and records their
// command paths.
func RecordToolCalls(t *testing.T, outcome func(path []string) (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError)) *[][]string {
	t.Helper()
	var calls [][]string
	prev := startToolCall
	t.Cleanup(func() { startToolCall = prev })
	startToolCall = func(_ string, path []string, input types.TypedSchemaValue, _ io.Reader, _ Streams) (toolCall, error) {
		root := input.Graph.TypeNodes[input.Graph.Root].Body
		if root.Tag() != types.SchemaTypeBodyRecordType {
			t.Errorf("the input graph is not rooted at a record")
		}
		calls = append(calls, path)
		return toolCall{wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			return outcome(path)
		}, cancel: func() {}}, nil
	}
	return &calls
}

// RecordToolInputs routes tool calls to an empty result and records the inputs
// they carry.
func RecordToolInputs(t *testing.T) *[]types.TypedSchemaValue {
	t.Helper()
	var inputs []types.TypedSchemaValue
	prev := startToolCall
	t.Cleanup(func() { startToolCall = prev })
	startToolCall = func(_ string, _ []string, input types.TypedSchemaValue, _ io.Reader, _ Streams) (toolCall, error) {
		inputs = append(inputs, input)
		return toolCall{wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			return witTypes.None[types.TypedSchemaValue](), nil
		}, cancel: func() {}}, nil
	}
	return &inputs
}

var _ golem.Unit
