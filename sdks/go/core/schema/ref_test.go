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

import "testing"

// str is a pointer helper, since optional fields are pointers in this model.

func typ(b SchemaTypeBody) SchemaType { return SchemaType{Body: b} }

// node builds a graph with one named definition, which is how a recursive type
// has to be expressed.
func nodeGraph(id string, body SchemaType, root SchemaType) SchemaGraph {
	return SchemaGraph{
		Defs: []SchemaTypeDef{{Id: id, Body: body}},
		Root: root,
	}
}

func TestRefResolvesThroughNamedDefinitions(t *testing.T) {
	g := nodeGraph("demo.Name", typ(StringType{}), typ(RefType{Id: "demo.Name"}))
	got, err := NewRef(g).Resolved()
	if err != nil {
		t.Fatalf("Resolved: %v", err)
	}
	if _, ok := got.Type().Body.(StringType); !ok {
		t.Errorf("resolved to %T, want StringType", got.Type().Body)
	}
}

// TestRefReportsAnUnknownDefinition — a dangling reference is a malformed
// graph, and saying so beats resolving to something arbitrary.
func TestRefReportsAnUnknownDefinition(t *testing.T) {
	g := SchemaGraph{Root: typ(RefType{Id: "demo.Missing"})}
	if _, err := NewRef(g).Resolved(); err == nil {
		t.Fatal("a dangling reference resolved")
	}
}

// TestRefTerminatesOnAReferenceCycle — a reference straight back to itself has
// no type in between, so following it must stop rather than hang.
func TestRefTerminatesOnAReferenceCycle(t *testing.T) {
	g := SchemaGraph{
		Defs: []SchemaTypeDef{
			{Id: "a", Body: typ(RefType{Id: "b"})},
			{Id: "b", Body: typ(RefType{Id: "a"})},
		},
		Root: typ(RefType{Id: "a"}),
	}
	if _, err := NewRef(g).Resolved(); err == nil {
		t.Fatal("a reference cycle resolved instead of being reported")
	}
}

func TestContainsStreamFindsNestedStreams(t *testing.T) {
	stream := typ(StreamType{Item: &SchemaType{Body: StringType{}}})
	for _, tc := range []struct {
		name string
		root SchemaType
		want bool
	}{
		{"the stream itself", stream, true},
		{"inside a record", typ(RecordType{Fields: []NamedField{{Name: "s", Body: stream}}}), true},
		{"inside an option", typ(OptionType{Inner: stream}), true},
		{"inside a result's error side", typ(ResultType{Err: &stream}), true},
		{"inside a map value", typ(MapType{Key: typ(StringType{}), Value: stream}), true},
		{"a plain record", typ(RecordType{Fields: []NamedField{{Name: "n", Body: typ(S32Type{})}}}), false},
	} {
		g := SchemaGraph{Root: tc.root}
		if got := NewRef(g).ContainsStream(); got != tc.want {
			t.Errorf("%s: ContainsStream=%v, want %v", tc.name, got, tc.want)
		}
	}
}

// TestContainsStreamTerminatesOnRecursion — a recursive type must not make the
// walk loop; a cycle cannot introduce a stream the first visit did not see.
func TestContainsStreamTerminatesOnRecursion(t *testing.T) {
	g := nodeGraph("demo.Node",
		typ(RecordType{Fields: []NamedField{
			{Name: "next", Body: typ(ListType{Element: typ(RefType{Id: "demo.Node"})})},
		}}),
		typ(RefType{Id: "demo.Node"}))
	if NewRef(g).ContainsStream() {
		t.Error("a recursive record without a stream reported one")
	}
}

func TestTypeNameRendersComposites(t *testing.T) {
	for _, tc := range []struct {
		root SchemaType
		want string
	}{
		{typ(StringType{}), "string"},
		{typ(ListType{Element: typ(S64Type{})}), "list<s64>"},
		{typ(FixedListType{Element: typ(U8Type{}), Length: 4}), "list<u8; 4>"},
		{typ(MapType{Key: typ(StringType{}), Value: typ(BoolType{})}), "map<string, bool>"},
		{typ(OptionType{Inner: typ(TextType{})}), "option<text>"},
		{typ(ResultType{Ok: &SchemaType{Body: S32Type{}}}), "result<s32, _>"},
		{typ(TupleType{Elements: []SchemaType{typ(StringType{}), typ(BoolType{})}}), "tuple<string, bool>"},
		{typ(QuantityType{Spec: QuantitySpec{BaseUnit: "kg"}}), "quantity<kg>"},
		{typ(EnumType{Cases: []string{"a", "b"}}), "enum{a, b}"},
		{typ(StreamType{Item: &SchemaType{Body: StringType{}}}), "stream<string>"},
		{typ(StreamType{}), "stream<?>"},
	} {
		g := SchemaGraph{Root: tc.root}
		if got := NewRef(g).TypeName(); got != tc.want {
			t.Errorf("TypeName = %q, want %q", got, tc.want)
		}
	}
}

// TestTypeNameStopsAtANamedDefinition — rendering follows the structure, so a
// recursive type would render forever if a name did not break the cycle.
func TestTypeNameStopsAtANamedDefinition(t *testing.T) {
	g := nodeGraph("demo.Node",
		typ(RecordType{Fields: []NamedField{{Name: "next", Body: typ(RefType{Id: "demo.Node"})}}}),
		typ(RefType{Id: "demo.Node"}))
	if got := NewRef(g).TypeName(); got != "demo.Node" {
		t.Errorf("TypeName = %q, want the definition's id", got)
	}
}
