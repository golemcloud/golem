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

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Unions are closed sum types whose branch is *inferred* from the value rather
// than carried alongside it. Where a [DefineVariant] value travels as a tag plus
// a payload, a union value travels as the branch body alone, and each branch
// declares the rule a decoder uses to recognise it:
//
//	type Shape interface{ isShape() }
//
//	type Circle struct{ Kind string; Radius float64 }
//	func (Circle) isShape() {}
//
//	type Square struct{ Kind string; Side float64 }
//	func (Square) isShape() {}
//
//	var _ = golem.DefineUnion[Shape](
//	    golem.Branch[Circle]("circle", golem.ByFieldEquals("kind", "circle")),
//	    golem.Branch[Square]("square", golem.ByFieldEquals("kind", "square")),
//	)
//
// Reach for a union when the wire format is fixed by something outside Golem —
// an existing JSON API, say — and for anything else prefer a variant, whose tag
// makes decoding unambiguous by construction.

// Discriminator is the rule that selects a union branch. Build one with
// [ByFieldEquals], [ByFieldPresent], [ByFieldAbsent], [ByPrefix], [BySuffix], [ByContains]
// or [ByRegex].
type Discriminator struct{ rule types.DiscriminatorRule }

// ByFieldEquals matches a record carrying field with exactly this literal value.
func ByFieldEquals(field, literal string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleFieldEquals(types.FieldDiscriminator{
		FieldName: field,
		Literal:   witTypes.Some(literal),
	})}
}

// ByFieldPresent matches a record carrying field, whatever its value.
func ByFieldPresent(field string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleFieldEquals(types.FieldDiscriminator{
		FieldName: field,
		Literal:   witTypes.None[string](),
	})}
}

// ByFieldAbsent matches a record that does not carry field.
func ByFieldAbsent(field string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleFieldAbsent(field)}
}

// ByPrefix matches a string value starting with the given text.
func ByPrefix(text string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRulePrefix(text)}
}

// BySuffix matches a string value ending with the given text.
func BySuffix(text string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleSuffix(text)}
}

// ByContains matches a string value containing the given text.
func ByContains(text string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleContains(text)}
}

// ByRegex matches a string value against a regular expression.
func ByRegex(pattern string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleRegex(pattern)}
}

// BranchDef is one branch of a union, produced by [Branch] or [WrappedBranch].
type BranchDef struct{ b engine.Branch }

// Branch declares a union branch: the body type T, recognised by the given
// discriminator and reported under tag.
func Branch[T any](tag string, discriminator Discriminator) BranchDef {
	return BranchDef{engine.Branch{Tag: tag, Typ: reflect.TypeFor[T](), Rule: discriminator.rule}}
}

// WrappedBranch declares a union branch whose body is the single field of T,
// for the same reason [WrappedCase] exists: a body such as a golem.Text or a
// string-prefixed identifier cannot carry the union's marker method itself
// without becoming a different type.
func WrappedBranch[T any](tag string, discriminator Discriminator) BranchDef {
	return BranchDef{engine.Branch{Tag: tag, Typ: reflect.TypeFor[T](), Rule: discriminator.rule, Wrapped: true}}
}

// DefineUnion registers the closed set of types inhabiting the interface Iface
// as an inferred-tag union. Branch order is declaration order and is the order a
// decoder tries them, so reordering the arguments can change which branch an
// ambiguous value resolves to. Call it from a package-level var so registration
// happens before the component is invoked.
func DefineUnion[Iface any](branches ...BranchDef) *engine.UnionDef {
	return defineUnionInto[Iface](defs, branches...)
}

// defineUnionInto is the instance-scoped implementation behind DefineUnion.
func defineUnionInto[Iface any](d *definitions, branches ...BranchDef) *engine.UnionDef {
	bs := make([]engine.Branch, len(branches))
	for i, b := range branches {
		bs[i] = b.b
	}
	return d.DefineUnion(reflect.TypeFor[Iface](), bs)
}
