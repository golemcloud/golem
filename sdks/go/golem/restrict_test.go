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
	"math"
	"slices"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type restrictedIn struct {
	Width  uint32           `golem:"min=1,max=4096,unit=px"`
	Offset int16            `golem:"min=-5"`
	Ratio  float64          `golem:"max=2.5"`
	Name   Text             `golem:"languages=en|de,minLength=1,regex=^[a-z]{1,3},[0-9]$"`
	Photo  Binary           `golem:"mime=image/png|image/jpeg,maxBytes=1024"`
	Out    Path             `golem:"direction=output,kind=file,extensions=png"`
	Source URL              `golem:"schemes=https,hosts=example.com"`
	Weight Quantity[kgUnit] `golem:"min=0.5,max=20kg"`
	Limit  Option[uint8]    `golem:"max=9"`
	Plain  string
}

type kgUnit struct{}

func (kgUnit) BaseUnit() string          { return "kg" }
func (kgUnit) AllowedSuffixes() []string { return nil }

func paramNode(t *testing.T, d *definitions, agent, method, param string) types.SchemaTypeBody {
	t.Helper()
	at, _ := d.buildAgentType(d.agents[agent])
	for _, m := range at.Methods {
		if m.Name != method {
			continue
		}
		for _, f := range m.InputSchema.Parameters() {
			if f.Name == param {
				return at.Schema.TypeNodes[f.Schema].Body
			}
		}
	}
	t.Fatalf("no parameter %s.%s.%s", agent, method, param)
	return types.SchemaTypeBody{}
}

func TestRestrictionTagsLandInTheSchema(t *testing.T) {
	type Id struct{ Name string }
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "R"})
		m := def.Method[restrictedIn, Unit]("resize")
		impl := implementInto[Id, struct{}, NoConfig](d, def, simpleNewState[Id, struct{}](func(Id) *struct{} { return &struct{}{} }), false)
		impl.Handle(m, func(*Context[struct{}], restrictedIn) Unit { return Unit{} })
		noDefErrs(t, d)
		node := func(param string) types.SchemaTypeBody { return paramNode(t, d, "R", "resize", param) }

		width := node("width").U32Type().Some()
		if width.Min.Some().Unsigned() != 1 || width.Max.Some().Unsigned() != 4096 || width.Unit.Some() != "px" {
			t.Errorf("width restrictions %+v", width)
		}
		if offset := node("offset").S16Type().Some(); offset.Min.Some().Signed() != -5 || offset.Max.IsSome() {
			t.Errorf("offset restrictions %+v", offset)
		}
		if ratio := node("ratio").F64Type().Some(); math.Float64frombits(ratio.Max.Some().FloatBits()) != 2.5 {
			t.Errorf("ratio restrictions %+v", ratio)
		}
		name := node("name").TextType()
		if !slices.Equal(name.Languages.Some(), []string{"en", "de"}) || name.MinLength.Some() != 1 || name.Regex.Some() != "^[a-z]{1,3},[0-9]$" {
			t.Errorf("name restrictions %+v", name)
		}
		if photo := node("photo").BinaryType(); !slices.Equal(photo.MimeTypes.Some(), []string{"image/png", "image/jpeg"}) || photo.MaxBytes.Some() != 1024 {
			t.Errorf("photo restrictions %+v", photo)
		}
		if out := node("out").PathType(); out.Direction != types.PathDirectionOutput || out.Kind != types.PathKindFile || !slices.Equal(out.AllowedExtensions.Some(), []string{"png"}) {
			t.Errorf("out restrictions %+v", out)
		}
		if source := node("source").UrlType(); !slices.Equal(source.AllowedSchemes.Some(), []string{"https"}) || !slices.Equal(source.AllowedHosts.Some(), []string{"example.com"}) {
			t.Errorf("source restrictions %+v", source)
		}
		weight := node("weight").QuantityType()
		if min := weight.Min.Some(); min.Mantissa != 5 || min.Scale != 1 || min.Unit != "kg" {
			t.Errorf("weight minimum %+v", min)
		}
		if max := weight.Max.Some(); max.Mantissa != 20 || max.Unit != "kg" {
			t.Errorf("weight maximum %+v", max)
		}

		// An optional field is restricted through to its value.
		at, _ := d.buildAgentType(d.agents["R"])
		limit := node("limit")
		if limit.Tag() != types.SchemaTypeBodyOptionType {
			t.Fatalf("limit is tag %d", limit.Tag())
		}
		if inner := at.Schema.TypeNodes[limit.OptionType()].Body.U8Type().Some(); inner.Max.Some().Unsigned() != 9 {
			t.Errorf("limit restrictions %+v", inner)
		}
		if node("plain").Tag() != types.SchemaTypeBodyStringType {
			t.Error("an untagged field changed")
		}
	})
}

func TestARestrictionThatCannotApplyIsADefinitionError(t *testing.T) {
	type Id struct{ Name string }
	type wrongKind struct {
		S string `golem:"min=1"`
	}
	type unknownKey struct {
		S Text `golem:"colour=red"`
	}
	type noValue struct {
		N uint8 `golem:"min"`
	}
	check := func(want string, declare func(*AgentDefinition[Id, NoConfig], *AgentImpl[Id, struct{}, NoConfig])) {
		withDefs(t, func(d *definitions) {
			def := defineAgentInto[Id, NoConfig](d, Spec{Name: "R"})
			impl := implementInto[Id, struct{}, NoConfig](d, def, simpleNewState[Id, struct{}](func(Id) *struct{} { return &struct{}{} }), false)
			declare(def, impl)
			mustDefErr(t, d, want)
		})
	}
	check("use golem.Text", func(def *AgentDefinition[Id, NoConfig], impl *AgentImpl[Id, struct{}, NoConfig]) {
		impl.Handle(def.Method[wrongKind, Unit]("m"), func(*Context[struct{}], wrongKind) Unit { return Unit{} })
	})
	check("unknown restriction", func(def *AgentDefinition[Id, NoConfig], impl *AgentImpl[Id, struct{}, NoConfig]) {
		impl.Handle(def.Method[unknownKey, Unit]("m"), func(*Context[struct{}], unknownKey) Unit { return Unit{} })
	})
	check("key=value", func(def *AgentDefinition[Id, NoConfig], impl *AgentImpl[Id, struct{}, NoConfig]) {
		impl.Handle(def.Method[noValue, Unit]("m"), func(*Context[struct{}], noValue) Unit { return Unit{} })
	})
}

type Shaper struct{}

type ShapeArgs struct {
	Size  uint32
	Label Text
	Tags  []Text
	Seed  Option[int64]
	Depth Option[uint8] `golem:"max=3"`
}

func TestToolArgumentSettersRestrictTheSchema(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	tool := defineToolInto[Shaper](r, d, "shaper", ToolSpec{Version: "1.0.0", Summary: "Shapes"}, false)
	shape := tool.Command[ShapeArgs, Unit]("shape", func(a *ShapeArgs, s *ToolCommandSpec) {
		s.Positional(&a.Size).Range(1, 64).Unit("px")
		s.Option(&a.Label).Regex("^[a-z]+$").MaxLength(12)
		s.List(&a.Tags).Languages("en")
		s.Option(&a.Seed).MinValue(Some[int64](-1))
		s.Option(&a.Depth)
	})
	shape.Handle(func(*ToolContext, ShapeArgs) (Unit, error) { return Unit{}, nil })
	if _, ok := r.discover(d); !ok {
		t.Fatalf("tool discovery failed: %s", allDefErrors(d.Errs))
	}
	e, _ := r.get("shaper")
	built, _ := d.buildTool(e)
	body := &built.Schema
	cmd := built.Commands.Nodes[1].Body.Some()
	size := body.TypeNodes[cmd.Positionals.Fixed[0].Type].Body.U32Type().Some()
	if size.Min.Some().Unsigned() != 1 || size.Max.Some().Unsigned() != 64 || size.Unit.Some() != "px" {
		t.Errorf("size restrictions %+v", size)
	}
	for _, o := range cmd.Options {
		switch o.Long {
		case "label":
			label := body.TypeNodes[o.Shape.Scalar()].Body.TextType()
			if label.Regex.Some() != "^[a-z]+$" || label.MaxLength.Some() != 12 {
				t.Errorf("label restrictions %+v", label)
			}
		case "tags":
			tags := body.TypeNodes[o.Shape.RepeatableList().ItemType].Body.TextType()
			if !slices.Equal(tags.Languages.Some(), []string{"en"}) {
				t.Errorf("tags restrictions %+v", tags)
			}
		case "depth":
			depth := body.TypeNodes[o.Shape.Scalar()].Body.U8Type().Some()
			if depth.Max.Some().Unsigned() != 3 {
				t.Errorf("a tagged argument lost its restriction: %+v", depth)
			}
		case "seed":
			seed := body.TypeNodes[o.Shape.Scalar()].Body.S64Type().Some()
			if seed.Min.Some().Signed() != -1 {
				t.Errorf("seed restrictions %+v", seed)
			}
		}
	}
}

func TestAToolArgumentRestrictionThatCannotApplyFailsTheDefinition(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	tool := defineToolInto[Shaper](r, d, "shaper", ToolSpec{Version: "1.0.0", Summary: "Shapes"}, false)
	tool.Command[ShapeArgs, Unit]("shape", func(a *ShapeArgs, s *ToolCommandSpec) {
		s.Positional(&a.Size).Regex("^[0-9]+$")
	})
	if _, ok := r.discover(d); ok || !containsDefErr(d.Errs, "restriction regex does not apply to a number") {
		t.Fatalf("errors: %s", allDefErrors(d.Errs))
	}
}
