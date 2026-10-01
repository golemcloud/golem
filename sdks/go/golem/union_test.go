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
	"reflect"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type Shape interface{ isShape() }

type Circle struct {
	Kind   string
	Radius float64
}

func (Circle) isShape() {}

type Square struct {
	Kind string
	Side float64
}

func (Square) isShape() {}

var _ = DefineUnion[Shape](
	Branch[Circle]("circle", FieldEquals("kind", "circle")),
	Branch[Square]("square", FieldEquals("kind", "square")),
)

func TestUnionRoundTrips(t *testing.T) {
	assertRoundTrip[Shape](t, "circle", Circle{Kind: "circle", Radius: 1.5})
	assertRoundTrip[Shape](t, "square", Square{Kind: "square", Side: 2})
	assertRoundTrip(t, "option<union>", Some[Shape](Circle{Kind: "circle", Radius: 3}))
}

// TestUnionCarriesBranchTagsAndRules — the tag travels with the value so a
// receiver need not re-run the discriminator, and the rules travel with the
// type so one still can.
func TestUnionCarriesBranchTagsAndRules(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Shape]()))
	body := g.build().TypeNodes[root].Body
	if body.Tag() != types.SchemaTypeBodyUnionType {
		t.Fatalf("Shape lowered to tag %d, want union-type", body.Tag())
	}
	branches := body.UnionType().Branches
	if len(branches) != 2 {
		t.Fatalf("union has %d branches, want 2", len(branches))
	}
	if branches[0].Tag != "circle" || branches[1].Tag != "square" {
		t.Errorf("branch tags %q/%q, want circle/square", branches[0].Tag, branches[1].Tag)
	}
	rule := branches[0].Discriminator
	if rule.Tag() != types.DiscriminatorRuleFieldEquals {
		t.Fatalf("discriminator tag %d, want field-equals", rule.Tag())
	}
	if d := rule.FieldEquals(); d.FieldName != "kind" || !d.Literal.IsSome() || d.Literal.Some() != "circle" {
		t.Errorf("discriminator is %+v, want kind == circle", d)
	}
}

func TestDefineUnionRejectsNonInterface(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineUnionInto[Circle](d, Branch[Circle]("circle", FieldPresent("kind")))
		mustDefErr(t, d, "DefineUnion requires an interface type")
	})
}

// TestDefineUnionRejectsNonImplementingBranch — without the check the branch
// would be unreachable at encode time, with the failure surfacing far from the
// declaration.
func TestDefineUnionRejectsNonImplementingBranch(t *testing.T) {
	type Stranger struct{ Kind string }
	withDefs(t, func(d *definitions) {
		defineUnionInto[Shape](d, Branch[Stranger]("stranger", FieldPresent("kind")))
		mustDefErr(t, d, "does not implement")
	})
}

func TestDefineUnionRejectsDuplicateTags(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineUnionInto[Shape](d,
			Branch[Circle]("shape", FieldEquals("kind", "circle")),
			Branch[Square]("shape", FieldEquals("kind", "square")),
		)
		mustDefErr(t, d, "duplicate branch tag")
	})
}

// TestDefineUnionRejectsAVariantInterface — the two encodings are different, so
// one interface cannot be both.
func TestDefineUnionRejectsAVariantInterface(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineVariantInto[Shape](d, Case[Circle]("circle"))
		defineUnionInto[Shape](d, Branch[Circle]("circle", FieldPresent("kind")))
		mustDefErr(t, d, "already defined as a variant")
	})
}

func TestDiscriminatorRules(t *testing.T) {
	for _, tc := range []struct {
		name string
		d    Discriminator
		want uint8
	}{
		{"FieldEquals", FieldEquals("kind", "circle"), types.DiscriminatorRuleFieldEquals},
		{"FieldPresent", FieldPresent("kind"), types.DiscriminatorRuleFieldEquals},
		{"FieldAbsent", FieldAbsent("kind"), types.DiscriminatorRuleFieldAbsent},
		{"Prefix", Prefix("re-"), types.DiscriminatorRulePrefix},
		{"Suffix", Suffix("-v2"), types.DiscriminatorRuleSuffix},
		{"Contains", Contains("::"), types.DiscriminatorRuleContains},
		{"Matches", Matches("^a+$"), types.DiscriminatorRuleRegex},
	} {
		if got := tc.d.rule.Tag(); got != tc.want {
			t.Errorf("%s produced tag %d, want %d", tc.name, got, tc.want)
		}
	}
	// FieldPresent is field-equals with no literal: the field must be there,
	// whatever it holds.
	if d := FieldPresent("kind").rule.FieldEquals(); d.Literal.IsSome() {
		t.Errorf("FieldPresent carries a literal %q", d.Literal.Some())
	}
}
