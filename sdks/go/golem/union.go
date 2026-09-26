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
	"fmt"
	"reflect"

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
//	    golem.Branch[Circle]("circle", golem.FieldEquals("kind", "circle")),
//	    golem.Branch[Square]("square", golem.FieldEquals("kind", "square")),
//	)
//
// Reach for a union when the wire format is fixed by something outside Golem —
// an existing JSON API, say — and for anything else prefer a variant, whose tag
// makes decoding unambiguous by construction.

// Discriminator is the rule that selects a union branch. Build one with
// [FieldEquals], [FieldPresent], [FieldAbsent], [Prefix], [Suffix], [Contains]
// or [Matches].
type Discriminator struct{ rule types.DiscriminatorRule }

// FieldEquals matches a record carrying field with exactly this literal value.
func FieldEquals(field, literal string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleFieldEquals(types.FieldDiscriminator{
		FieldName: field,
		Literal:   witTypes.Some(literal),
	})}
}

// FieldPresent matches a record carrying field, whatever its value.
func FieldPresent(field string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleFieldEquals(types.FieldDiscriminator{
		FieldName: field,
		Literal:   witTypes.None[string](),
	})}
}

// FieldAbsent matches a record that does not carry field.
func FieldAbsent(field string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleFieldAbsent(field)}
}

// Prefix matches a string value starting with the given text.
func Prefix(text string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRulePrefix(text)}
}

// Suffix matches a string value ending with the given text.
func Suffix(text string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleSuffix(text)}
}

// Contains matches a string value containing the given text.
func Contains(text string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleContains(text)}
}

// Matches matches a string value against a regular expression.
func Matches(pattern string) Discriminator {
	return Discriminator{types.MakeDiscriminatorRuleRegex(pattern)}
}

// BranchDef is one branch of a union, produced by [Branch] or [WrappedBranch].
type BranchDef struct {
	tag           string
	typ           reflect.Type
	discriminator Discriminator
	// wrapped means typ is a one-field struct whose field is the body.
	wrapped bool
}

// Branch declares a union branch: the body type T, recognised by the given
// discriminator and reported under tag.
func Branch[T any](tag string, discriminator Discriminator) BranchDef {
	return BranchDef{tag: tag, typ: reflect.TypeFor[T](), discriminator: discriminator}
}

// WrappedBranch declares a union branch whose body is the single field of T,
// for the same reason [WrappedCase] exists: a body such as a golem.Text or a
// string-prefixed identifier cannot carry the union's marker method itself
// without becoming a different type.
func WrappedBranch[T any](tag string, discriminator Discriminator) BranchDef {
	return BranchDef{tag: tag, typ: reflect.TypeFor[T](), discriminator: discriminator, wrapped: true}
}

// unionDef is the registered branch list for one interface type. Branch order is
// declaration order and is the order a decoder tries them, so reordering
// [DefineUnion]'s arguments can change which branch an ambiguous value resolves
// to.
type unionDef struct {
	iface    reflect.Type
	branches []BranchDef
}

// DefineUnion registers the closed set of types inhabiting the interface Iface
// as an inferred-tag union. Call it from a package-level var so registration
// happens before the component is invoked.
func DefineUnion[Iface any](branches ...BranchDef) *unionDef {
	return defineUnionInto[Iface](defs, branches...)
}

// defineUnionInto is the instance-scoped implementation behind DefineUnion.
func defineUnionInto[Iface any](d *definitions, branches ...BranchDef) *unionDef {
	it := reflect.TypeFor[Iface]()
	ud := &unionDef{iface: it, branches: branches}
	if it.Kind() != reflect.Interface {
		d.recordErr("", "", "DefineUnion requires an interface type, got %s", it)
		return ud
	}
	if _, dup := d.unions[it]; dup {
		d.recordErr("", "", "union already defined for %s", it)
		return ud
	}
	if _, dup := d.variants[it]; dup {
		d.recordErr("", "", "%s is already defined as a variant; it cannot also be a union", it)
		return ud
	}
	if len(branches) == 0 {
		d.recordErr("", "", "DefineUnion[%s] needs at least one branch", it)
	}
	seen := map[string]bool{}
	for _, b := range branches {
		if seen[b.tag] {
			d.recordErr("", "", "DefineUnion[%s]: duplicate branch tag %q", it, b.tag)
		}
		seen[b.tag] = true
		if b.wrapped {
			if reason := wrappedPayloadErr(b.typ); reason != "" {
				d.recordErr("", "", "DefineUnion[%s]: wrapped branch %q: %s", it, b.tag, reason)
			}
		}
		if !b.typ.Implements(it) {
			d.recordErr("", "", "DefineUnion[%s]: branch %q has type %s, which does not implement %s",
				it, b.tag, b.typ, it)
		}
	}
	d.unions[it] = ud
	return ud
}

// compileUnion lowers a registered union. A value encodes as its branch body
// plus the resolved tag, which the receiver uses instead of re-running the
// discriminator rules.
func (d *definitions) compileUnion(c *codec, ud *unionDef) {
	branchCodecs := make([]*codec, len(ud.branches))
	byType := make(map[reflect.Type]int, len(ud.branches))
	for i, b := range ud.branches {
		branchCodecs[i] = d.compile(payloadType(b.typ, b.wrapped))
		byType[b.typ] = i
	}

	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		out := make([]types.UnionBranch, 0, len(ud.branches))
		for i, b := range ud.branches {
			out = append(out, types.UnionBranch{
				Tag:           b.tag,
				Body:          g.node(branchCodecs[i]),
				Discriminator: b.discriminator.rule,
				Metadata:      types.MetadataEnvelope{},
			})
		}
		return types.MakeSchemaTypeBodyUnionType(types.UnionSpec{Branches: out})
	}

	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		concrete := v
		if v.Kind() == reflect.Interface {
			if v.IsNil() {
				panic(&encodeError{fmt.Sprintf("nil %s cannot be encoded; a union must hold one of its branches", c.typ)})
			}
			concrete = v.Elem()
		}
		i, ok := byType[concrete.Type()]
		if !ok {
			panic(&encodeError{fmt.Sprintf("%s is not a registered branch of union %s", concrete.Type(), c.typ)})
		}
		body := branchCodecs[i].encode(b, payloadValue(concrete, ud.branches[i].wrapped))
		return b.push(types.MakeSchemaValueNodeUnionValue(types.UnionValuePayload{
			Tag:  ud.branches[i].tag,
			Body: body,
		}))
	}

	c.decode = func(dec *decoder, dst reflect.Value, idx int32) error {
		n, err := dec.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeUnionValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		p := n.UnionValue()
		for i, b := range ud.branches {
			if b.tag != p.Tag {
				continue
			}
			out := reflect.New(b.typ).Elem()
			if err := branchCodecs[i].decode(dec, payloadValue(out, b.wrapped), p.Body); err != nil {
				return fmt.Errorf("%s branch %q: %w", c.typ, b.tag, err)
			}
			dst.Set(out)
			return nil
		}
		return fmt.Errorf("%s: unknown union branch tag %q", c.typ, p.Tag)
	}
}
