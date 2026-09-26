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

package witschema

import (
	"testing"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// builder assembles a flat WIT graph the way the generated code does.
type builder struct {
	nodes []types.SchemaTypeNode
	defs  []types.SchemaTypeDef
}

func (b *builder) push(body types.SchemaTypeBody) int32 {
	b.nodes = append(b.nodes, types.SchemaTypeNode{Body: body})
	return int32(len(b.nodes) - 1)
}

func (b *builder) define(id string, body int32) int32 {
	b.defs = append(b.defs, types.SchemaTypeDef{
		Id: id, Name: witTypes.None[string](), Body: body,
	})
	return b.push(types.MakeSchemaTypeBodyRefType(int32(len(b.defs) - 1)))
}

func (b *builder) graph(root int32) types.SchemaGraph {
	return types.SchemaGraph{TypeNodes: b.nodes, Defs: b.defs, Root: root}
}

func noRestrictions() witTypes.Option[types.NumericRestrictions] {
	return witTypes.None[types.NumericRestrictions]()
}

// TestEveryWitBodyTagConverts is the guard that matters most here. Go switches
// are not exhaustive, so a case added to the WIT would otherwise fall through
// and convert to nothing. This walks every declared tag and insists the
// converter either handles it or says it cannot.
func TestEveryWitBodyTagConverts(t *testing.T) {
	// If this fails, the bindings gained or lost a case: handle it in body()
	// and move the count deliberately.
	const declared = witBodyTagCount
	if types.SchemaTypeBodyStreamType != declared-1 {
		t.Fatalf("the bindings declare tags up to %d, but the converter pins %d; "+
			"a WIT case was added or removed", types.SchemaTypeBodyStreamType, declared-1)
	}

	var b builder
	inner := b.push(types.MakeSchemaTypeBodyStringType())
	c := &converter{wit: b.graph(inner), byIndex: map[int32]core.SchemaType{}}

	// Every tag the bindings declare must be reachable; an unhandled one comes
	// back as an error rather than silently becoming nil.
	for tag := uint8(0); tag < declared; tag++ {
		body, ok := sampleBody(tag, inner)
		if !ok {
			continue
		}
		if _, err := c.body(body); err != nil {
			t.Errorf("tag %d did not convert: %v", tag, err)
		}
	}
}

// sampleBody builds a minimal body for a tag, using inner wherever a nested
// type is needed.
func sampleBody(tag uint8, inner int32) (types.SchemaTypeBody, bool) {
	switch tag {
	case types.SchemaTypeBodyRefType:
		return types.SchemaTypeBody{}, false // needs a def; covered separately
	case types.SchemaTypeBodyBoolType:
		return types.MakeSchemaTypeBodyBoolType(), true
	case types.SchemaTypeBodyS8Type:
		return types.MakeSchemaTypeBodyS8Type(noRestrictions()), true
	case types.SchemaTypeBodyS16Type:
		return types.MakeSchemaTypeBodyS16Type(noRestrictions()), true
	case types.SchemaTypeBodyS32Type:
		return types.MakeSchemaTypeBodyS32Type(noRestrictions()), true
	case types.SchemaTypeBodyS64Type:
		return types.MakeSchemaTypeBodyS64Type(noRestrictions()), true
	case types.SchemaTypeBodyU8Type:
		return types.MakeSchemaTypeBodyU8Type(noRestrictions()), true
	case types.SchemaTypeBodyU16Type:
		return types.MakeSchemaTypeBodyU16Type(noRestrictions()), true
	case types.SchemaTypeBodyU32Type:
		return types.MakeSchemaTypeBodyU32Type(noRestrictions()), true
	case types.SchemaTypeBodyU64Type:
		return types.MakeSchemaTypeBodyU64Type(noRestrictions()), true
	case types.SchemaTypeBodyF32Type:
		return types.MakeSchemaTypeBodyF32Type(noRestrictions()), true
	case types.SchemaTypeBodyF64Type:
		return types.MakeSchemaTypeBodyF64Type(noRestrictions()), true
	case types.SchemaTypeBodyCharType:
		return types.MakeSchemaTypeBodyCharType(), true
	case types.SchemaTypeBodyStringType:
		return types.MakeSchemaTypeBodyStringType(), true
	case types.SchemaTypeBodyRecordType:
		return types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
			{Name: "f", Body: inner},
		}), true
	case types.SchemaTypeBodyVariantType:
		return types.MakeSchemaTypeBodyVariantType([]types.VariantCaseType{
			{Name: "c", Payload: witTypes.Some(inner)},
		}), true
	case types.SchemaTypeBodyEnumType:
		return types.MakeSchemaTypeBodyEnumType([]string{"a"}), true
	case types.SchemaTypeBodyFlagsType:
		return types.MakeSchemaTypeBodyFlagsType([]string{"a"}), true
	case types.SchemaTypeBodyTupleType:
		return types.MakeSchemaTypeBodyTupleType([]int32{inner}), true
	case types.SchemaTypeBodyListType:
		return types.MakeSchemaTypeBodyListType(inner), true
	case types.SchemaTypeBodyFixedListType:
		return types.MakeSchemaTypeBodyFixedListType(types.FixedListSpec{Element: inner, Length: 2}), true
	case types.SchemaTypeBodyMapType:
		return types.MakeSchemaTypeBodyMapType(types.MapSpec{Key: inner, Value: inner}), true
	case types.SchemaTypeBodyOptionType:
		return types.MakeSchemaTypeBodyOptionType(inner), true
	case types.SchemaTypeBodyResultType:
		return types.MakeSchemaTypeBodyResultType(types.ResultSpec{
			Ok: witTypes.Some(inner), Err: witTypes.None[int32](),
		}), true
	case types.SchemaTypeBodyTextType:
		return types.MakeSchemaTypeBodyTextType(types.TextRestrictions{
			Languages: witTypes.None[[]string](), MinLength: witTypes.None[uint32](),
			MaxLength: witTypes.None[uint32](), Regex: witTypes.None[string](),
		}), true
	case types.SchemaTypeBodyBinaryType:
		return types.MakeSchemaTypeBodyBinaryType(types.BinaryRestrictions{
			MimeTypes: witTypes.None[[]string](), MinBytes: witTypes.None[uint32](),
			MaxBytes: witTypes.None[uint32](),
		}), true
	case types.SchemaTypeBodyPathType:
		return types.MakeSchemaTypeBodyPathType(types.PathSpec{
			Direction: types.PathDirectionInOut, Kind: types.PathKindAny,
			AllowedMimeTypes: witTypes.None[[]string](), AllowedExtensions: witTypes.None[[]string](),
		}), true
	case types.SchemaTypeBodyUrlType:
		return types.MakeSchemaTypeBodyUrlType(types.UrlRestrictions{
			AllowedSchemes: witTypes.None[[]string](), AllowedHosts: witTypes.None[[]string](),
		}), true
	case types.SchemaTypeBodyDatetimeType:
		return types.MakeSchemaTypeBodyDatetimeType(), true
	case types.SchemaTypeBodyDurationType:
		return types.MakeSchemaTypeBodyDurationType(), true
	case types.SchemaTypeBodyQuantityType:
		return types.MakeSchemaTypeBodyQuantityType(types.QuantitySpec{
			BaseUnit: "B", Min: witTypes.None[types.QuantityValue](),
			Max: witTypes.None[types.QuantityValue](),
		}), true
	case types.SchemaTypeBodyUnionType:
		return types.MakeSchemaTypeBodyUnionType(types.UnionSpec{Branches: []types.UnionBranch{{
			Tag: "t", Body: inner, Discriminator: types.MakeDiscriminatorRulePrefix("x"),
		}}}), true
	case types.SchemaTypeBodySecretType:
		return types.MakeSchemaTypeBodySecretType(types.SecretSpec{
			Inner: inner, Category: witTypes.None[string](),
		}), true
	case types.SchemaTypeBodyQuotaTokenType:
		return types.MakeSchemaTypeBodyQuotaTokenType(types.QuotaTokenSpec{
			ResourceName: witTypes.None[string](),
		}), true
	case types.SchemaTypeBodyPermissionCardType:
		return types.MakeSchemaTypeBodyPermissionCardType(types.PermissionCardSpec{}), true
	case types.SchemaTypeBodyFutureType:
		return types.MakeSchemaTypeBodyFutureType(witTypes.Some(inner)), true
	case types.SchemaTypeBodyStreamType:
		return types.MakeSchemaTypeBodyStreamType(witTypes.Some(inner)), true
	}
	return types.SchemaTypeBody{}, false
}

// TestRecursionBecomesANamedReference — the flat form expresses recursion with
// a ref node into the def pool; the recursive form names the definition. The
// conversion must terminate either way.
func TestRecursionBecomesANamedReference(t *testing.T) {
	var b builder
	str := b.push(types.MakeSchemaTypeBodyStringType())
	placeholder := b.push(types.MakeSchemaTypeBodyBoolType())
	ref := b.define("demo.Node", placeholder)
	children := b.push(types.MakeSchemaTypeBodyListType(ref))
	b.nodes[placeholder] = types.SchemaTypeNode{
		Body: types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
			{Name: "name", Body: str},
			{Name: "children", Body: children},
		}),
	}

	got, err := GraphToCore(b.graph(ref))
	if err != nil {
		t.Fatalf("GraphToCore: %v", err)
	}
	if len(got.Graph.Defs) != 1 || got.Graph.Defs[0].Id != "demo.Node" {
		t.Fatalf("defs are %+v", got.Graph.Defs)
	}
	root, ok := got.Graph.Root.Body.(core.RefType)
	if !ok || root.Id != "demo.Node" {
		t.Fatalf("root is %#v, want a reference to demo.Node", got.Graph.Root.Body)
	}
	// The whole point: the converted graph renders without looping.
	if name := core.NewRef(got.Graph).TypeName(); name != "demo.Node" {
		t.Errorf("TypeName = %q", name)
	}
}

// TestIndexSideTableSurvivesConversion — reflection selects sub-schemas by WIT
// node index, and the recursive form has none, so the mapping has to be kept.
func TestIndexSideTableSurvivesConversion(t *testing.T) {
	var b builder
	str := b.push(types.MakeSchemaTypeBodyStringType())
	num := b.push(types.MakeSchemaTypeBodyS32Type(noRestrictions()))
	root := b.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "name", Body: str},
		{Name: "count", Body: num},
	}))

	got, err := GraphToCore(b.graph(root))
	if err != nil {
		t.Fatalf("GraphToCore: %v", err)
	}
	at, err := got.At(num)
	if err != nil {
		t.Fatalf("At(%d): %v", num, err)
	}
	if _, ok := at.Body.(core.S32Type); !ok {
		t.Errorf("index %d resolved to %T, want S32Type", num, at.Body)
	}
	if _, err := got.At(999); err == nil {
		t.Error("an index outside the graph resolved")
	}
}

func TestConversionRejectsAMalformedGraph(t *testing.T) {
	var b builder
	root := b.push(types.MakeSchemaTypeBodyListType(42)) // no such node
	if _, err := GraphToCore(b.graph(root)); err == nil {
		t.Error("a graph referring to a missing node converted")
	}
}

// TestMetadataAndRestrictionsSurvive — the details are what a reader uses, so
// dropping them silently would be worse than failing.
func TestMetadataAndRestrictionsSurvive(t *testing.T) {
	var b builder
	root := b.push(types.MakeSchemaTypeBodyTextType(types.TextRestrictions{
		Languages: witTypes.Some([]string{"en"}),
		MinLength: witTypes.Some(uint32(1)),
		MaxLength: witTypes.None[uint32](),
		Regex:     witTypes.None[string](),
	}))
	b.nodes[root].Metadata = types.MetadataEnvelope{
		Doc:        witTypes.Some("a name"),
		Deprecated: witTypes.Some("use fullName"),
	}

	got, err := GraphToCore(b.graph(root))
	if err != nil {
		t.Fatalf("GraphToCore: %v", err)
	}
	text, ok := got.Graph.Root.Body.(core.TextType)
	if !ok {
		t.Fatalf("root is %T", got.Graph.Root.Body)
	}
	if text.Restrictions.Languages == nil || (*text.Restrictions.Languages)[0] != "en" {
		t.Errorf("languages did not survive: %v", text.Restrictions.Languages)
	}
	if text.Restrictions.MinLength == nil || *text.Restrictions.MinLength != 1 {
		t.Errorf("min length did not survive")
	}
	if text.Restrictions.MaxLength != nil {
		t.Errorf("an absent restriction became present")
	}
	md := got.Graph.Root.Metadata
	if md.Doc == nil || *md.Doc != "a name" || md.Deprecated == nil {
		t.Errorf("metadata did not survive: %+v", md)
	}
}
