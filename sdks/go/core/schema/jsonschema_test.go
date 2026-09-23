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
	"strings"
	"testing"
)

func renderDoc(t *testing.T, g SchemaGraph, draft bool) map[string]any {
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

func assertJSON(t *testing.T, what string, got any, want string) {
	t.Helper()
	gotBytes, _ := json.Marshal(got)
	var wantAny any
	if err := json.Unmarshal([]byte(want), &wantAny); err != nil {
		t.Fatalf("%s: want is not valid JSON: %v", what, err)
	}
	wantBytes, _ := json.Marshal(wantAny)
	if string(gotBytes) != string(wantBytes) {
		t.Errorf("%s:\n got: %s\nwant: %s", what, gotBytes, wantBytes)
	}
}

// TestJSONSchemaDescribesTheCanonicalJSON — the document and the codec must
// agree, or a reader told "integer" sends a number the host refuses.
func TestJSONSchemaDescribesTheCanonicalJSON(t *testing.T) {
	for _, tc := range []struct {
		name string
		root SchemaType
		want string
	}{
		{"s64 is a string", typ(S64Type{}), `{
			"type":"string","format":"int64",
			"pattern":"^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$",
			"x-golem-minimum":"-9223372036854775808",
			"x-golem-maximum":"9223372036854775807"}`},
		{"u64 is a string", typ(U64Type{}), `{
			"type":"string","format":"uint64",
			"pattern":"^(?:0|[1-9][0-9]*)$",
			"x-golem-minimum":"0","x-golem-maximum":"18446744073709551615"}`},
		{"s32 stays a number", typ(S32Type{}), `{
			"type":"integer","minimum":-2147483648,"maximum":2147483647}`},
		{"flags are unique names", typ(FlagsType{Flags: []string{"read", "write"}}), `{
			"type":"array","items":{"type":"string","enum":["read","write"]},
			"uniqueItems":true}`},
	} {
		got := renderDoc(t, SchemaGraph{Root: tc.root}, false)
		assertJSON(t, tc.name, got, tc.want)
	}
}

func TestJSONSchemaDurationIsAnObject(t *testing.T) {
	got := renderDoc(t, SchemaGraph{Root: typ(DurationType{})}, false)
	if got["type"] != "object" || got["additionalProperties"] != false {
		t.Fatalf("duration rendered as %v", got)
	}
	props, _ := got["properties"].(map[string]any)
	nanos, _ := props["nanoseconds"].(map[string]any)
	if nanos["type"] != "string" || nanos["format"] != "int64" {
		t.Errorf("nanoseconds rendered as %v, want a canonical int64 string", nanos)
	}
}

func TestJSONSchemaMapIsAnArrayOfPairs(t *testing.T) {
	root := typ(MapType{Key: typ(S32Type{}), Value: typ(StringType{})})
	assertJSON(t, "map", renderDoc(t, SchemaGraph{Root: root}, false), `{
		"type":"array",
		"items":{"type":"array",
			"prefixItems":[{"type":"integer","minimum":-2147483648,"maximum":2147483647},
				{"type":"string"}],
			"items":false,"minItems":2,"maxItems":2}}`)
}

// TestJSONSchemaOptionFieldsAreNotRequired — an option field may be omitted
// entirely, so listing it as required would reject valid payloads.
func TestJSONSchemaOptionFieldsAreNotRequired(t *testing.T) {
	root := typ(RecordType{Fields: []NamedField{
		{Name: "name", Body: typ(StringType{})},
		{Name: "nickname", Body: typ(OptionType{Inner: typ(StringType{})})},
	}})
	got := renderDoc(t, SchemaGraph{Root: root}, false)
	assertJSON(t, "required", got["required"], `["name"]`)
}

// TestJSONSchemaNamedDefsBecomeRefs — a named type is emitted once under $defs
// and referenced, which is also what terminates a recursive type.
func TestJSONSchemaNamedDefsBecomeRefs(t *testing.T) {
	g := nodeGraph("golem.it.Node",
		typ(RecordType{Fields: []NamedField{
			{Name: "name", Body: typ(StringType{})},
			{Name: "children", Body: typ(ListType{Element: typ(RefType{Id: "golem.it.Node"})})},
		}}),
		typ(RefType{Id: "golem.it.Node"}))

	got := renderDoc(t, g, false)
	assertJSON(t, "root", got["$ref"], `"#/$defs/golem.it.Node"`)
	defs, _ := got["$defs"].(map[string]any)
	if _, ok := defs["golem.it.Node"]; !ok {
		t.Fatalf("$defs is missing the named type: %v", defs)
	}
}

// TestJSONSchemaRefPointersAreEscaped — RFC 6901 reserves ~ and / in a
// pointer, so an id containing them has to be escaped or the pointer dangles.
func TestJSONSchemaRefPointersAreEscaped(t *testing.T) {
	if got := refPointer("a/b~c"); got != "#/$defs/a~1b~0c" {
		t.Errorf("refPointer = %q, want %q", got, "#/$defs/a~1b~0c")
	}
}

func TestJSONSchemaDraftMarkerIsOptional(t *testing.T) {
	g := SchemaGraph{Root: typ(BoolType{})}
	if got := renderDoc(t, g, true); got["$schema"] != jsonSchemaDraft {
		t.Errorf("draft marker missing: %v", got["$schema"])
	}
	if got := renderDoc(t, g, false); got["$schema"] != nil {
		t.Errorf("draft marker present when not asked for: %v", got["$schema"])
	}
}

// TestJSONSchemaCapabilitiesAreNotConstructible — a capability is supplied by
// the platform, so a caller must not be invited to send one.
func TestJSONSchemaCapabilitiesAreNotConstructible(t *testing.T) {
	root := typ(SecretType{Inner: typ(StringType{})})
	assertJSON(t, "secret", renderDoc(t, SchemaGraph{Root: root}, false),
		`{"writeOnly":true,"x-golem-capability":"secret"}`)
}

func TestJSONSchemaMetadataIsAttached(t *testing.T) {
	doc := "the customer's display name"
	dep := "use fullName"
	root := SchemaType{
		Body: StringType{},
		Metadata: MetadataEnvelope{
			Doc: &doc, Examples: []string{`"ada"`}, Deprecated: &dep,
		},
	}
	assertJSON(t, "metadata", renderDoc(t, SchemaGraph{Root: root}, false), `{
		"type":"string","description":"the customer's display name",
		"examples":["ada"],"deprecated":true}`)
}

// TestJSONSchemaUnionBranchesCarryTheirDiscriminator — the oneOf has to be
// decidable, so each branch is narrowed by the rule that selects it.
func TestJSONSchemaUnionBranchesCarryTheirDiscriminator(t *testing.T) {
	kind := "circle"
	root := typ(UnionType{Branches: []UnionBranch{{
		Tag:           "circle",
		Body:          typ(RecordType{Fields: []NamedField{{Name: "kind", Body: typ(StringType{})}}}),
		Discriminator: FieldEqualsRule{FieldName: "kind", Literal: &kind},
	}}})
	got := renderDoc(t, SchemaGraph{Root: root}, false)
	one, _ := got["oneOf"].([]any)
	if len(one) != 1 {
		t.Fatalf("oneOf has %d branches", len(one))
	}
	branch, _ := one[0].(map[string]any)
	all, _ := branch["allOf"].([]any)
	if len(all) != 2 {
		t.Fatalf("branch is %v, want the body narrowed by its condition", branch)
	}
}

func TestParametersRenderAsAnObject(t *testing.T) {
	g := SchemaGraph{Root: typ(BoolType{})}
	params := []Parameter{
		{Name: "name", Type: typ(StringType{})},
		{Name: "nickname", Type: typ(OptionType{Inner: typ(StringType{})})},
		BoolParameter("force"),
	}
	rendered, err := NewRef(g).ParametersJSONSchema(params, true)
	if err != nil {
		t.Fatalf("ParametersJSONSchema: %v", err)
	}
	data, _ := json.Marshal(rendered)
	var doc map[string]any
	_ = json.Unmarshal(data, &doc)

	if doc["type"] != "object" || doc["additionalProperties"] != false {
		t.Errorf("rendered as %v", doc)
	}
	required, _ := doc["required"].([]any)
	if len(required) != 2 {
		t.Errorf("required = %v, want the two non-option parameters", required)
	}
	props, _ := doc["properties"].(map[string]any)
	force, _ := props["force"].(map[string]any)
	if force["type"] != "boolean" {
		t.Errorf("a fixed-type parameter rendered as %v", force)
	}
}

func TestPackParametersBuildsTheInvocationRecord(t *testing.T) {
	g := SchemaGraph{Root: typ(BoolType{})}
	params := []Parameter{
		{Name: "greeting", Type: typ(StringType{})},
		{Name: "times", Type: typ(S32Type{})},
		BoolParameter("loud"),
	}
	v, err := NewRef(g).PackParameters(params, map[string]any{
		"greeting": "hi", "times": 2, "loud": true,
	})
	if err != nil {
		t.Fatalf("PackParameters: %v", err)
	}
	record, ok := v.(RecordValue)
	if !ok || len(record.Fields) != 3 {
		t.Fatalf("packed to %#v", v)
	}

	back, err := NewRef(g).UnpackParameters(params, v)
	if err != nil {
		t.Fatalf("UnpackParameters: %v", err)
	}
	if back["greeting"] != "hi" || back["times"] != int64(2) || back["loud"] != true {
		t.Errorf("round trip gave %v", back)
	}
}

// TestPackParametersReportsEveryProblemAtOnce — a caller assembling arguments
// should learn about all of its mistakes together, not one per attempt.
func TestPackParametersReportsEveryProblemAtOnce(t *testing.T) {
	g := SchemaGraph{Root: typ(BoolType{})}
	params := []Parameter{
		{Name: "greeting", Type: typ(StringType{})},
		{Name: "times", Type: typ(S32Type{})},
	}
	_, err := NewRef(g).PackParameters(params, map[string]any{"greetng": "hi", "extra": 1})
	if err == nil {
		t.Fatal("packing malformed arguments succeeded")
	}
	msg := err.Error()
	for _, want := range []string{"greetng", "extra", "greeting", "times"} {
		if !strings.Contains(msg, want) {
			t.Errorf("message does not mention %q: %s", want, msg)
		}
	}
}
