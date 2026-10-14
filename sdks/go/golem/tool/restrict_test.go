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
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"slices"
	"testing"
)

type Shaper struct{}

type ShapeArgs struct {
	Size  uint32
	Label golem.Text
	Tags  []golem.Text
	Seed  golem.Option[int64]
	Depth golem.Option[uint8] `golem:"max=3"`
}

func TestToolArgumentSettersRestrictTheSchema(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	tool := defineToolInto[Shaper](r, d, "shaper", Spec{Version: "1.0.0", Summary: "Shapes"}, false)
	shape := tool.Command[ShapeArgs, golem.Unit]("shape", func(a *ShapeArgs, s *CommandSpec) {
		s.Positional(&a.Size).Range(1, 64).Unit("px")
		s.Option(&a.Label).Regex("^[a-z]+$").MaxLength(12)
		s.List(&a.Tags).Languages("en")
		s.Option(&a.Seed).MinValue(golem.Some[int64](-1))
		s.Option(&a.Depth)
	})
	shape.Handle(func(*Context, ShapeArgs) (golem.Unit, error) { return golem.Unit{}, nil })
	if _, ok := r.discover(d); !ok {
		t.Fatalf("tool discovery failed: %s", engine.AllErrors(d.Errs))
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
	tool := defineToolInto[Shaper](r, d, "shaper", Spec{Version: "1.0.0", Summary: "Shapes"}, false)
	tool.Command[ShapeArgs, golem.Unit]("shape", func(a *ShapeArgs, s *CommandSpec) {
		s.Positional(&a.Size).Regex("^[0-9]+$")
	})
	if _, ok := r.discover(d); ok || !containsDefErr(d.Errs, "restriction regex does not apply to a number") {
		t.Fatalf("errors: %s", engine.AllErrors(d.Errs))
	}
}
