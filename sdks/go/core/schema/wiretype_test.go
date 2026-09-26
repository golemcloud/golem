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

package schema

import (
	"fmt"
	"reflect"
	"strings"
	"testing"
)

// everyTypeKind is one wire node per SchemaType case, paired with the body it
// must read back as. Adding a case to the model without adding one here fails
// TestEveryTypeKindIsAccountedFor rather than silently landing in the
// "unsupported" arm at runtime.
func everyTypeKind() []struct {
	wire string
	want SchemaTypeBody
} {
	return []struct {
		wire string
		want SchemaTypeBody
	}{
		{`{"kind":"ref","value":{"id":"t1"}}`, RefType{Id: "t1"}},
		{`{"kind":"bool","value":{}}`, BoolType{}},
		{`{"kind":"char","value":{}}`, CharType{}},
		{`{"kind":"string","value":{}}`, StringType{}},
		{`{"kind":"s8","value":{}}`, S8Type{}},
		{`{"kind":"s16","value":{}}`, S16Type{}},
		{`{"kind":"s32","value":{}}`, S32Type{}},
		{`{"kind":"s64","value":{}}`, S64Type{}},
		{`{"kind":"u8","value":{}}`, U8Type{}},
		{`{"kind":"u16","value":{}}`, U16Type{}},
		{`{"kind":"u32","value":{}}`, U32Type{}},
		{`{"kind":"u64","value":{}}`, U64Type{}},
		{`{"kind":"f32","value":{}}`, F32Type{}},
		{`{"kind":"f64","value":{}}`, F64Type{}},
		{
			`{"kind":"record","value":{"fields":[{"name":"a","body":{"kind":"bool","value":{}}}]}}`,
			RecordType{Fields: []NamedField{{Name: "a", Body: SchemaType{Body: BoolType{}}}}},
		},
		{
			`{"kind":"variant","value":{"cases":[{"name":"none"},` +
				`{"name":"some","payload":{"kind":"string","value":{}}}]}}`,
			VariantType{Cases: []VariantCase{
				{Name: "none"},
				{Name: "some", Payload: &SchemaType{Body: StringType{}}},
			}},
		},
		{`{"kind":"enum","value":{"cases":["red","green"]}}`, EnumType{Cases: []string{"red", "green"}}},
		{`{"kind":"flags","value":{"flags":["a","b"]}}`, FlagsType{Flags: []string{"a", "b"}}},
		{
			`{"kind":"tuple","value":{"elements":[{"kind":"bool","value":{}}]}}`,
			TupleType{Elements: []SchemaType{{Body: BoolType{}}}},
		},
		{
			`{"kind":"list","value":{"element":{"kind":"u8","value":{}}}}`,
			ListType{Element: SchemaType{Body: U8Type{}}},
		},
		{
			`{"kind":"fixed-list","value":{"element":{"kind":"u8","value":{}},"length":4}}`,
			FixedListType{Element: SchemaType{Body: U8Type{}}, Length: 4},
		},
		{
			`{"kind":"map","value":{"key":{"kind":"string","value":{}},"value":{"kind":"bool","value":{}}}}`,
			MapType{Key: SchemaType{Body: StringType{}}, Value: SchemaType{Body: BoolType{}}},
		},
		{
			`{"kind":"option","value":{"inner":{"kind":"string","value":{}}}}`,
			OptionType{Inner: SchemaType{Body: StringType{}}},
		},
		{
			`{"kind":"result","value":{"spec":{"ok":{"kind":"bool","value":{}}}}}`,
			ResultType{Ok: &SchemaType{Body: BoolType{}}},
		},
		{
			`{"kind":"text","value":{"restrictions":{"minLength":1}}}`,
			TextType{Restrictions: TextRestrictions{MinLength: ptr(uint32(1))}},
		},
		{
			`{"kind":"binary","value":{"restrictions":{"mimeTypes":["image/png"]}}}`,
			BinaryType{Restrictions: BinaryRestrictions{MimeTypes: ptr([]string{"image/png"})}},
		},
		{
			`{"kind":"path","value":{"spec":{"direction":"in-out","kind":"directory"}}}`,
			PathType{Spec: PathSpec{Direction: PathInOut, Kind: PathDirectory}},
		},
		{
			`{"kind":"url","value":{"restrictions":{"allowedSchemes":["https"]}}}`,
			UrlType{Restrictions: UrlRestrictions{AllowedSchemes: ptr([]string{"https"})}},
		},
		{`{"kind":"datetime","value":{}}`, DatetimeType{}},
		{`{"kind":"duration","value":{}}`, DurationType{}},
		{
			`{"kind":"quantity","value":{"spec":{"baseUnit":"kg","allowedSuffixes":["g"]}}}`,
			QuantityType{Spec: QuantitySpec{BaseUnit: "kg", AllowedSuffixes: []string{"g"}}},
		},
		{
			`{"kind":"union","value":{"spec":{"branches":[{"tag":"inline",` +
				`"body":{"kind":"string","value":{}},"discriminator":{"rule":"prefix","value":{"prefix":"x"}}}]}}}`,
			UnionType{Branches: []UnionBranch{{
				Tag:           "inline",
				Body:          SchemaType{Body: StringType{}},
				Discriminator: PrefixRule{Value: "x"},
			}}},
		},
		{
			`{"kind":"secret","value":{"spec":{"inner":{"kind":"string","value":{}},"category":"api-key"}}}`,
			SecretType{Inner: SchemaType{Body: StringType{}}, Category: ptr("api-key")},
		},
		{
			`{"kind":"quota-token","value":{"spec":{"resourceName":"tokens"}}}`,
			QuotaTokenType{ResourceName: ptr("tokens")},
		},
		{
			`{"kind":"permission-card","value":{"spec":{"polymorphic":true}}}`,
			PermissionCardType{Polymorphic: true},
		},
		{
			`{"kind":"future","value":{"inner":{"kind":"bool","value":{}}}}`,
			FutureType{Item: &SchemaType{Body: BoolType{}}},
		},
		{`{"kind":"stream","value":{"inner":null}}`, StreamType{}},
	}
}

func TestEveryTypeKindIsAccountedFor(t *testing.T) {
	kinds := everyTypeKind()
	if len(kinds) != wireTypeKinds {
		t.Fatalf("the wire codec knows %d type kinds, the test lists %d", wireTypeKinds, len(kinds))
	}
	seen := map[reflect.Type]bool{}
	for _, c := range kinds {
		typ := reflect.TypeOf(c.want)
		if seen[typ] {
			t.Fatalf("%v is listed twice", typ)
		}
		seen[typ] = true

		got, err := wireToType([]byte(c.wire))
		if err != nil {
			t.Fatalf("%s: %v", c.wire, err)
		}
		if !reflect.DeepEqual(got.Body, c.want) {
			t.Fatalf("%s read as %#v, want %#v", c.wire, got.Body, c.want)
		}
	}
}

func TestGraphDefinitionsAndRecursionAreResolved(t *testing.T) {
	// A list of itself: the only recursion the wire form allows is through a
	// named definition, so reading one has to keep the reference intact rather
	// than trying to inline it.
	graph, err := UnmarshalWireGraph([]byte(`{
		"defs": [{"id":"node","name":"Node","body":{"kind":"record","value":{"fields":[
			{"name":"next","body":{"kind":"option","value":{"inner":{"kind":"ref","value":{"id":"node"}}}}}
		]}}}],
		"root": {"kind":"ref","value":{"id":"node"}}
	}`))
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(graph.Defs) != 1 || graph.Defs[0].Id != "node" || *graph.Defs[0].Name != "Node" {
		t.Fatalf("definitions read as %#v", graph.Defs)
	}
	ref := NewRef(graph)
	resolved, err := ref.Resolved()
	if err != nil {
		t.Fatalf("resolve: %v", err)
	}
	if _, ok := resolved.Type().Body.(RecordType); !ok {
		t.Fatalf("root resolved to %T, want a record", resolved.Type().Body)
	}
}

func TestMetadataAndRolesSurvive(t *testing.T) {
	graph, err := UnmarshalWireGraph([]byte(`{"root":{"kind":"string","value":{"metadata":{
		"doc":"a name","aliases":["alias"],"examples":["\"x\""],
		"deprecated":"use id","role":{"tag":"unstructured-text"}}}}}`))
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	m := graph.Root.Metadata
	if m.Doc == nil || *m.Doc != "a name" {
		t.Fatalf("doc read as %v", m.Doc)
	}
	if !reflect.DeepEqual(m.Aliases, []string{"alias"}) ||
		!reflect.DeepEqual(m.Examples, []string{`"x"`}) {
		t.Fatalf("aliases/examples read as %v / %v", m.Aliases, m.Examples)
	}
	if m.Deprecated == nil || *m.Deprecated != "use id" {
		t.Fatalf("deprecated read as %v", m.Deprecated)
	}
	if m.Role == nil || *m.Role != RoleUnstructuredText {
		t.Fatalf("role read as %v", m.Role)
	}
}

// A role this build does not know is an open registry entry, not an error: the
// producer's intent survives as its own name.
func TestAnUnknownRoleKeepsItsName(t *testing.T) {
	graph, err := UnmarshalWireGraph([]byte(
		`{"root":{"kind":"string","value":{"metadata":{"role":{"tag":"other","value":"chart"}}}}}`))
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if graph.Root.Metadata.Role == nil || *graph.Root.Metadata.Role != Role("chart") {
		t.Fatalf("role read as %v", graph.Root.Metadata.Role)
	}
}

// The server's SecretSpec defaults its payload type to string when absent.
func TestASecretWithoutAnInnerTypeIsASecretString(t *testing.T) {
	got, err := wireToType([]byte(`{"kind":"secret","value":{"spec":{"category":"api-key"}}}`))
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	secret, ok := got.Body.(SecretType)
	if !ok {
		t.Fatalf("read as %T", got.Body)
	}
	if _, ok := secret.Inner.Body.(StringType); !ok {
		t.Fatalf("inner read as %T, want a string", secret.Inner.Body)
	}
}

func TestEveryDiscriminatorRuleIsRead(t *testing.T) {
	cases := []struct {
		wire string
		want DiscriminatorRule
	}{
		{`{"rule":"prefix","value":{"prefix":"p"}}`, PrefixRule{Value: "p"}},
		{`{"rule":"suffix","value":{"suffix":"s"}}`, SuffixRule{Value: "s"}},
		{`{"rule":"contains","value":{"substring":"c"}}`, ContainsRule{Value: "c"}},
		{`{"rule":"regex","value":{"regex":"^r$"}}`, RegexRule{Pattern: "^r$"}},
		{`{"rule":"field-equals","value":{"fieldName":"kind","literal":"a"}}`,
			FieldEqualsRule{FieldName: "kind", Literal: ptr("a")}},
		{`{"rule":"field-absent","value":{"fieldName":"kind"}}`,
			FieldAbsentRule{FieldName: "kind"}},
	}
	seen := map[reflect.Type]bool{}
	for _, c := range cases {
		wire := fmt.Sprintf(
			`{"kind":"union","value":{"spec":{"branches":[{"tag":"t",`+
				`"body":{"kind":"string","value":{}},"discriminator":%s}]}}}`, c.wire)
		got, err := wireToType([]byte(wire))
		if err != nil {
			t.Fatalf("%s: %v", c.wire, err)
		}
		rule := got.Body.(UnionType).Branches[0].Discriminator
		if !reflect.DeepEqual(rule, c.want) {
			t.Fatalf("%s read as %#v, want %#v", c.wire, rule, c.want)
		}
		seen[reflect.TypeOf(c.want)] = true
	}
	if len(seen) != 6 {
		t.Fatalf("the model has 6 discriminator rules, the test covers %d", len(seen))
	}
}

func TestNumericRestrictionsKeepTheirWidth(t *testing.T) {
	got, err := wireToType([]byte(
		`{"kind":"u64","value":{"restrictions":{"min":{"kind":"unsigned","value":0},` +
			`"max":{"kind":"unsigned","value":18446744073709551615},"unit":"B"}}}`))
	if err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	r := got.Body.(U64Type).Restrictions
	if r == nil || r.Max == nil || r.Max.Kind != BoundUnsigned ||
		r.Max.Unsigned != 18446744073709551615 {
		t.Fatalf("max read as %#v", r)
	}
	if r.Unit == nil || *r.Unit != "B" {
		t.Fatalf("unit read as %v", r.Unit)
	}
}

func TestMalformedTypesAreRejected(t *testing.T) {
	cases := map[string]string{
		`{"kind":"nope","value":{}}`: "unsupported schema type kind",
		`{"kind":"u64","value":{"restrictions":{"min":{"kind":"huge","value":0}}}}`:     "numeric bound kind",
		`{"kind":"u64","value":{"restrictions":{"min":{"kind":"signed","value":"x"}}}}`: "numeric bound",
		`{"kind":"path","value":{"spec":{"direction":"sideways","kind":"file"}}}`:       "path direction",
		`{"kind":"path","value":{"spec":{"direction":"input","kind":"socket"}}}`:        "path kind",
		`{"kind":"union","value":{"spec":{"branches":[{"tag":"t",` +
			`"body":{"kind":"string","value":{}},"discriminator":{"rule":"nope"}}]}}}`: "discriminator rule",
		`{"kind":"list","value":{}}`: "missing type",
	}
	for wire, want := range cases {
		_, err := wireToType([]byte(wire))
		if err == nil || !strings.Contains(err.Error(), want) {
			t.Fatalf("%s: got %v, want an error mentioning %q", wire, err, want)
		}
	}
}
