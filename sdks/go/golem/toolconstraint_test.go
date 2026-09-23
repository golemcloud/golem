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
	"testing"

	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/schema"
)

type ExportArgs struct {
	Path  Positional[string]
	JSON  Flag
	YAML  Flag
	Sign  Flag
	Key   Opt[string]
	Level Opt[int32]
}

// TestConstraintsReachTheCommandContract — the rules are published with the
// command, so a caller can reject a bad invocation before making it.
func TestConstraintsReachTheCommandContract(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "export", ToolSpec{Version: "1.0.0"})
		cmd := declareCommand[ExportArgs, string](r, d, def, nil, "", ExportArgs{}, []CommandOpt{
			Constrain(
				Mutex(Present("json"), Present("yaml")),
				Implies(AllOf(Present("sign")), AnyOf(Present("key"))),
				RequiresAll(Present("path")),
				AllOrNone(Present("json"), Present("level")),
				RequiresAny(Present("json"), Present("yaml")),
				Forbids(AllOf(Present("sign")), Present("yaml")),
			),
		})
		handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) string { return "" })
	})

	cs := tool.Commands.Nodes[0].Body.Some().Constraints
	if len(cs) != 6 {
		t.Fatalf("command publishes %d constraints, want 6", len(cs))
	}
	want := []uint8{
		toolCommon.ConstraintMutexGroups,
		toolCommon.ConstraintImplies,
		toolCommon.ConstraintRequiresAll,
		toolCommon.ConstraintAllOrNone,
		toolCommon.ConstraintRequiresAny,
		toolCommon.ConstraintForbids,
	}
	for i, w := range want {
		if cs[i].Tag() != w {
			t.Errorf("constraint %d has tag %d, want %d", i, cs[i].Tag(), w)
		}
	}

	// Mutex is one group of two.
	groups := cs[0].MutexGroups()
	if len(groups) != 1 || len(groups[0].Refs) != 2 {
		t.Errorf("mutex is %+v, want one group of two", groups)
	}
	implies := cs[1].Implies()
	if implies.LhsQuant != toolCommon.QuantifierAll || implies.RhsQuant != toolCommon.QuantifierAny {
		t.Errorf("implies quantifiers are %d/%d, want all/any", implies.LhsQuant, implies.RhsQuant)
	}
}

// TestValueIsCarriesAnEncodedLiteral — the comparison value travels as a schema
// value interpreted against the referenced argument's own type.
func TestValueIsCarriesAnEncodedLiteral(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "level", ToolSpec{Version: "1.0.0"})
		cmd := declareCommand[ExportArgs, string](r, d, def, nil, "", ExportArgs{}, []CommandOpt{
			Constrain(Implies(AllOf(ValueIs("level", int32(3))), AnyOf(Present("key")))),
		})
		handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) string { return "" })
	})

	implies := tool.Commands.Nodes[0].Body.Some().Constraints[0].Implies()
	if len(implies.Lhs) != 1 || implies.Lhs[0].Tag() != toolCommon.RefValueIs {
		t.Fatalf("lhs is %+v, want a value-is reference", implies.Lhs)
	}
	vr := implies.Lhs[0].ValueIs()
	if vr.Name != "level" {
		t.Errorf("reference names %q, want level", vr.Name)
	}
	// The literal is read against the option's declared type node.
	optType := tool.Commands.Nodes[0].Body.Some().Options[1].Shape.Scalar()
	got, err := schema.NewRef(tool.Schema).WithRoot(optType).UnpackJSON(vr.Value)
	if err != nil {
		t.Fatalf("literal is not readable: %v", err)
	}
	if got != int64(3) {
		t.Errorf("literal is %v, want 3", got)
	}
}

func TestConstraintDeclarationErrors(t *testing.T) {
	declare := func(t *testing.T, opts ...CommandOpt) *definitions {
		t.Helper()
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "bad", ToolSpec{})
		cmd := declareCommand[ExportArgs, string](r, d, def, nil, "", ExportArgs{}, opts)
		handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) string { return "" })
		r.discover(d)
		return d
	}

	t.Run("unknown argument", func(t *testing.T) {
		d := declare(t, Constrain(Mutex(Present("json"), Present("nope"))))
		mustDefErr(t, d, `constrains "nope", which it does not declare`)
	})

	t.Run("literal of the wrong type", func(t *testing.T) {
		d := declare(t, Constrain(RequiresAll(ValueIs("level", "three"))))
		mustDefErr(t, d, "compares \"level\" against a string")
	})
}

// TestFormattersAndTheirDefault — a tool declares the renderings it offers; the
// default has to be one of them.
func TestFormattersAndTheirDefault(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "fmt", ToolSpec{Version: "1.0.0"})
		cmd := declareCommand[ExportArgs, string](r, d, def, nil, "", ExportArgs{}, []CommandOpt{
			ResultDoc("the exported document"),
			Formats(
				Formatter{Name: "json", Summary: "machine readable"},
				Formatter{Name: "table", Summary: "for humans"},
			),
			DefaultFormat("table"),
		})
		handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) string { return "" })
	})

	spec := tool.Commands.Nodes[0].Body.Some().Result.Some()
	if spec.Doc.Summary != "the exported document" {
		t.Errorf("result doc %q", spec.Doc.Summary)
	}
	if len(spec.Formatters) != 2 || spec.Formatters[0].Name != "json" {
		t.Fatalf("formatters are %+v", spec.Formatters)
	}
	if spec.DefaultFormatter != "table" {
		t.Errorf("default formatter %q, want table", spec.DefaultFormatter)
	}
}

// TestFirstFormatterIsTheDefault — declaring one rendering and no default is
// unambiguous, and the WIT requires the field to name a declared formatter.
func TestFirstFormatterIsTheDefault(t *testing.T) {
	tool, _, _ := buildToolFor(t, func(r *toolRegistry, d *definitions) {
		def := defineToolInto(r, d, "fmt2", ToolSpec{Version: "1.0.0"})
		cmd := declareCommand[ExportArgs, string](r, d, def, nil, "", ExportArgs{}, []CommandOpt{
			Formats(Formatter{Name: "json"}),
		})
		handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) string { return "" })
	})
	if got := tool.Commands.Nodes[0].Body.Some().Result.Some().DefaultFormatter; got != "json" {
		t.Errorf("default formatter %q, want json", got)
	}
}

func TestFormatterDeclarationErrors(t *testing.T) {
	declare := func(t *testing.T, out int, opts ...CommandOpt) *definitions {
		t.Helper()
		r, d := newToolRegistry(), newDefinitions()
		def := defineToolInto(r, d, "badfmt", ToolSpec{})
		if out == 1 {
			cmd := declareCommand[ExportArgs, Unit](r, d, def, nil, "", ExportArgs{}, opts)
			handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) Unit { return Unit{} })
		} else {
			cmd := declareCommand[ExportArgs, string](r, d, def, nil, "", ExportArgs{}, opts)
			handleCommandInto(r, d, cmd, func(*ToolContext, ExportArgs) string { return "" })
		}
		r.discover(d)
		return d
	}

	t.Run("default names an undeclared formatter", func(t *testing.T) {
		d := declare(t, 0, Formats(Formatter{Name: "json"}), DefaultFormat("table"))
		mustDefErr(t, d, "which it does not declare")
	})

	t.Run("duplicate formatter", func(t *testing.T) {
		d := declare(t, 0, Formats(Formatter{Name: "json"}, Formatter{Name: "json"}))
		mustDefErr(t, d, "twice")
	})

	t.Run("formatters without a result", func(t *testing.T) {
		d := declare(t, 1, Formats(Formatter{Name: "json"}))
		mustDefErr(t, d, "declares formatters but returns no result")
	})
}
