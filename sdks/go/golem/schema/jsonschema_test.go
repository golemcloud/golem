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
	"encoding/json"
	"testing"

	witTypes "go.bytecodealliance.org/pkg/wit/types"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// renderJSONSchema renders the graph rooted at root and decodes it back, so the
// assertions read against plain maps rather than the rendering types.
func renderJSONSchema(t *testing.T, g types.SchemaGraph, draft bool) map[string]any {
	t.Helper()
	data, err := NewRef(g).ToJSONSchemaBytes(draft)
	if err != nil {
		t.Fatalf("ToJSONSchemaBytes: %v", err)
	}
	var out map[string]any
	if err := json.Unmarshal(data, &out); err != nil {
		t.Fatalf("rendered schema is not valid JSON: %v", err)
	}
	return out
}

func assertJSONEqual(t *testing.T, what string, got any, want string) {
	t.Helper()
	gotBytes, err := json.Marshal(got)
	if err != nil {
		t.Fatalf("%s: marshal: %v", what, err)
	}
	var wantAny any
	if err := json.Unmarshal([]byte(want), &wantAny); err != nil {
		t.Fatalf("%s: want is not valid JSON: %v", what, err)
	}
	wantBytes, err := json.Marshal(wantAny)
	if err != nil {
		t.Fatalf("%s: remarshal: %v", what, err)
	}
	if string(gotBytes) != string(wantBytes) {
		t.Errorf("%s:\n got: %s\nwant: %s", what, gotBytes, wantBytes)
	}
}

// TestJSONSchemaWideIntegersAreStrings — the counterpart of the canonical-JSON
// rule: a reader told "integer" would send a JSON number, which the host
// rejects for s64/u64.
func TestJSONSchemaWideIntegersAreStrings(t *testing.T) {
	for _, tc := range []struct {
		name string
		body types.SchemaTypeBody
		want string
	}{
		{"s64", types.MakeSchemaTypeBodyS64Type(witTypes.None[types.NumericRestrictions]()), `{
			"type": "string", "format": "int64",
			"pattern": "^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$",
			"x-golem-minimum": "-9223372036854775808",
			"x-golem-maximum": "9223372036854775807"
		}`},
		{"u64", types.MakeSchemaTypeBodyU64Type(witTypes.None[types.NumericRestrictions]()), `{
			"type": "string", "format": "uint64",
			"pattern": "^(?:0|[1-9][0-9]*)$",
			"x-golem-minimum": "0",
			"x-golem-maximum": "18446744073709551615"
		}`},
		{"s32 stays a number", types.MakeSchemaTypeBodyS32Type(witTypes.None[types.NumericRestrictions]()), `{
			"type": "integer", "minimum": -2147483648, "maximum": 2147483647
		}`},
		{"u32 stays a number", types.MakeSchemaTypeBodyU32Type(witTypes.None[types.NumericRestrictions]()), `{
			"type": "integer", "minimum": 0, "maximum": 4294967295
		}`},
	} {
		var b graphBuilder
		root := b.push(tc.body)
		assertJSONEqual(t, tc.name, renderJSONSchema(t, b.graph(root), false), tc.want)
	}
}

// TestJSONSchemaDurationIsAnObject — a duration is an object carrying a
// nanoseconds string, not an ISO-8601 string.
func TestJSONSchemaDurationIsAnObject(t *testing.T) {
	var b graphBuilder
	root := b.push(types.MakeSchemaTypeBodyDurationType())
	got := renderJSONSchema(t, b.graph(root), false)
	if got["type"] != "object" {
		t.Fatalf("duration rendered as %v, want an object", got["type"])
	}
	props, _ := got["properties"].(map[string]any)
	nanos, _ := props["nanoseconds"].(map[string]any)
	if nanos["type"] != "string" || nanos["format"] != "int64" {
		t.Errorf("nanoseconds rendered as %v, want a canonical int64 string", nanos)
	}
	if got["additionalProperties"] != false {
		t.Errorf("duration allows additional properties")
	}
}

func TestJSONSchemaQuantityCarriesItsUnit(t *testing.T) {
	var b graphBuilder
	root := b.push(types.MakeSchemaTypeBodyQuantityType(types.QuantitySpec{
		BaseUnit:        "kg",
		AllowedSuffixes: []string{"kg", "g"},
		Min:             witTypes.None[types.QuantityValue](),
		Max:             witTypes.None[types.QuantityValue](),
	}))
	got := renderJSONSchema(t, b.graph(root), false)
	if got["title"] != "Quantity (kg)" {
		t.Errorf("title %v, want %q", got["title"], "Quantity (kg)")
	}
	props, _ := got["properties"].(map[string]any)
	mantissa, _ := props["mantissa"].(map[string]any)
	if mantissa["type"] != "string" {
		t.Errorf("mantissa rendered as %v, want a canonical string", mantissa["type"])
	}
}

// TestJSONSchemaOptionFieldsAreNotRequired — an option field may be omitted
// entirely, so listing it as required would reject valid payloads.
func TestJSONSchemaOptionFieldsAreNotRequired(t *testing.T) {
	var b graphBuilder
	str := b.push(types.MakeSchemaTypeBodyStringType())
	opt := b.push(types.MakeSchemaTypeBodyOptionType(str))
	root := b.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "name", Body: str, Metadata: types.MetadataEnvelope{}},
		{Name: "nickname", Body: opt, Metadata: types.MetadataEnvelope{}},
	}))
	got := renderJSONSchema(t, b.graph(root), false)
	assertJSONEqual(t, "required", got["required"], `["name"]`)
}

// TestJSONSchemaMapIsAnArrayOfPairs — map keys are not restricted to strings,
// so a map cannot render as a JSON object.
func TestJSONSchemaMapIsAnArrayOfPairs(t *testing.T) {
	var b graphBuilder
	key := b.push(types.MakeSchemaTypeBodyS32Type(witTypes.None[types.NumericRestrictions]()))
	val := b.push(types.MakeSchemaTypeBodyStringType())
	root := b.push(types.MakeSchemaTypeBodyMapType(types.MapSpec{Key: key, Value: val}))
	got := renderJSONSchema(t, b.graph(root), false)
	assertJSONEqual(t, "map", got, `{
		"type": "array",
		"items": {
			"type": "array",
			"prefixItems": [
				{"type": "integer", "minimum": -2147483648, "maximum": 2147483647},
				{"type": "string"}
			],
			"items": false, "minItems": 2, "maxItems": 2
		}
	}`)
}

func TestJSONSchemaTupleIsPositional(t *testing.T) {
	var b graphBuilder
	str := b.push(types.MakeSchemaTypeBodyStringType())
	flag := b.push(types.MakeSchemaTypeBodyBoolType())
	root := b.push(types.MakeSchemaTypeBodyTupleType([]int32{str, flag}))
	got := renderJSONSchema(t, b.graph(root), false)
	assertJSONEqual(t, "tuple", got, `{
		"type": "array",
		"prefixItems": [{"type": "string"}, {"type": "boolean"}],
		"items": false, "minItems": 2
	}`)
}

func TestJSONSchemaFlagsAreAUniqueNameArray(t *testing.T) {
	var b graphBuilder
	root := b.push(types.MakeSchemaTypeBodyFlagsType([]string{"read", "write"}))
	got := renderJSONSchema(t, b.graph(root), false)
	assertJSONEqual(t, "flags", got, `{
		"type": "array",
		"items": {"type": "string", "enum": ["read", "write"]},
		"uniqueItems": true
	}`)
}

// TestJSONSchemaNamedDefsBecomeRefs — a named type is emitted once under $defs
// and referenced, which is also what terminates a recursive type.
func TestJSONSchemaNamedDefsBecomeRefs(t *testing.T) {
	var b graphBuilder
	str := b.push(types.MakeSchemaTypeBodyStringType())
	// A self-referential list: Node = record { name: string, children: list<Node> }
	placeholder := b.push(types.MakeSchemaTypeBodyBoolType())
	ref := b.define("golem.it.Node", placeholder)
	children := b.push(types.MakeSchemaTypeBodyListType(ref))
	b.nodes[placeholder] = types.SchemaTypeNode{
		Body: types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
			{Name: "name", Body: str, Metadata: types.MetadataEnvelope{}},
			{Name: "children", Body: children, Metadata: types.MetadataEnvelope{}},
		}),
		Metadata: types.MetadataEnvelope{},
	}
	got := renderJSONSchema(t, b.graph(ref), false)
	assertJSONEqual(t, "root", got["$ref"], `"#/$defs/golem.it.Node"`)
	defs, _ := got["$defs"].(map[string]any)
	if _, ok := defs["golem.it.Node"]; !ok {
		t.Fatalf("$defs is missing the named type: %v", defs)
	}
}

// TestJSONSchemaRefPointersAreEscaped — RFC 6901 reserves `~` and `/` in a
// pointer, so an id containing them has to be escaped or the pointer dangles.
func TestJSONSchemaRefPointersAreEscaped(t *testing.T) {
	if got := refPointer("a/b~c"); got != "#/$defs/a~1b~0c" {
		t.Errorf("refPointer = %q, want %q", got, "#/$defs/a~1b~0c")
	}
}

func TestJSONSchemaDraftMarkerIsOptional(t *testing.T) {
	var b graphBuilder
	root := b.push(types.MakeSchemaTypeBodyBoolType())
	g := b.graph(root)
	if got := renderJSONSchema(t, g, true); got["$schema"] != jsonSchemaDraft {
		t.Errorf("draft marker missing: %v", got["$schema"])
	}
	if got := renderJSONSchema(t, g, false); got["$schema"] != nil {
		t.Errorf("draft marker present when not asked for: %v", got["$schema"])
	}
}

// TestJSONSchemaCapabilitiesAreNotConstructible — a secret is supplied by the
// platform, so a caller must not be invited to send one.
func TestJSONSchemaCapabilitiesAreNotConstructible(t *testing.T) {
	var b graphBuilder
	inner := b.push(types.MakeSchemaTypeBodyStringType())
	root := b.push(types.MakeSchemaTypeBodySecretType(types.SecretSpec{
		Inner:    inner,
		Category: witTypes.None[string](),
	}))
	got := renderJSONSchema(t, b.graph(root), false)
	assertJSONEqual(t, "secret", got, `{"writeOnly": true, "x-golem-capability": "secret"}`)
}

// TestJSONSchemaMetadataIsAttached — docs and examples ride on the node, so a
// generated tool schema explains itself.
func TestJSONSchemaMetadataIsAttached(t *testing.T) {
	var b graphBuilder
	root := b.push(types.MakeSchemaTypeBodyStringType())
	b.nodes[root].Metadata = types.MetadataEnvelope{
		Doc:        witTypes.Some("the customer's display name"),
		Examples:   []string{`"ada"`},
		Deprecated: witTypes.Some("use fullName"),
	}
	got := renderJSONSchema(t, b.graph(root), false)
	assertJSONEqual(t, "metadata", got, `{
		"type": "string",
		"description": "the customer's display name",
		"examples": ["ada"],
		"deprecated": true
	}`)
}
