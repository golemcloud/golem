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

// Bytes is the unit marker for a byte-count quantity.
type Bytes struct{}

func (Bytes) BaseUnit() string          { return "B" }
func (Bytes) AllowedSuffixes() []string { return nil }

// Weight accepts several suffixes alongside its base unit.
type Weight struct{}

func (Weight) BaseUnit() string          { return "kg" }
func (Weight) AllowedSuffixes() []string { return []string{"kg", "g", "mg"} }

type Perms struct{ Read, Write, Admin bool }

var _ = DefineFlags[Perms]()

// TestRichScalarsLowerToTheirOwnWitTypes — Text and Path are strings and Binary
// is a []byte underneath, so without recognition by named type they would
// silently lower to string, string and list<u8>.
func TestRichScalarsLowerToTheirOwnWitTypes(t *testing.T) {
	for _, tc := range []struct {
		name string
		rt   reflect.Type
		want uint8
	}{
		{"Text", reflect.TypeFor[Text](), types.SchemaTypeBodyTextType},
		{"Binary", reflect.TypeFor[Binary](), types.SchemaTypeBodyBinaryType},
		{"Path", reflect.TypeFor[Path](), types.SchemaTypeBodyPathType},
		{"Quantity", reflect.TypeFor[Quantity[Bytes]](), types.SchemaTypeBodyQuantityType},
		{"Tuple2", reflect.TypeFor[Tuple2[string, int64]](), types.SchemaTypeBodyTupleType},
		{"Perms", reflect.TypeFor[Perms](), types.SchemaTypeBodyFlagsType},
	} {
		g := graphBuilder{d: defs}
		root := g.node(defs.compile(tc.rt))
		if got := g.build().TypeNodes[root].Body.Tag(); got != tc.want {
			t.Errorf("%s lowered to tag %d, want %d", tc.name, got, tc.want)
		}
	}
}

func TestRichScalarsRoundTrip(t *testing.T) {
	assertRoundTrip(t, "text", Text("hello, world"))
	assertRoundTrip(t, "binary", Binary{0x00, 0x7f, 0xff})
	assertRoundTrip(t, "path", Path("/var/data/input.csv"))
	assertRoundTrip(t, "quantity", Quantity[Bytes]{Mantissa: 1500, Scale: 0, Unit: "B"})
	assertRoundTrip(t, "quantity with scale", Quantity[Weight]{Mantissa: 12345, Scale: 3, Unit: "kg"})

	// and they compose like everything else
	assertRoundTrip(t, "option<text>", Some(Text("maybe")))
	assertRoundTrip(t, "list<path>", []Path{"/a", "/b"})
}

// TestQuantityDefaultsToItsBaseUnit — an empty Unit is the common case of "the
// canonical unit", and the host rejects a unit that is neither the base unit nor
// a declared suffix, so it is filled in rather than sent empty.
func TestQuantityDefaultsToItsBaseUnit(t *testing.T) {
	got := roundTrip(t, Quantity[Bytes]{Mantissa: 42})
	if got.Unit != "B" {
		t.Errorf("empty unit encoded as %q, want the base unit %q", got.Unit, "B")
	}
}

// TestQuantitySpecComesFromTheUnitMarker — the constraints live on the type, not
// the value, so two quantities with different markers are different types.
func TestQuantitySpecComesFromTheUnitMarker(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Quantity[Weight]]()))
	spec := g.build().TypeNodes[root].Body.QuantityType()
	if spec.BaseUnit != "kg" {
		t.Errorf("base unit %q, want %q", spec.BaseUnit, "kg")
	}
	if !reflect.DeepEqual(spec.AllowedSuffixes, []string{"kg", "g", "mg"}) {
		t.Errorf("allowed suffixes %v, want [kg g mg]", spec.AllowedSuffixes)
	}
}

func TestTupleRoundTripsPositionally(t *testing.T) {
	assertRoundTrip(t, "tuple2", Tuple2[string, int64]{A: "x", B: 7})
	assertRoundTrip(t, "tuple3", Tuple3[bool, Text, []int32]{A: true, B: "t", C: []int32{1, 2}})
	assertRoundTrip(t, "nested", Tuple2[Tuple2[int32, int32], Option[string]]{
		A: Tuple2[int32, int32]{A: 1, B: 2},
		B: Some("inner"),
	})
}

// TestTupleIsNotARecord — the two are distinguishable on the wire: a tuple's
// elements are positional and carry no names.
func TestTupleIsNotARecord(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Tuple2[string, int64]]()))
	graph := g.build()
	elems := graph.TypeNodes[root].Body.TupleType()
	if len(elems) != 2 {
		t.Fatalf("tuple has %d elements, want 2", len(elems))
	}
	if tag := graph.TypeNodes[elems[0]].Body.Tag(); tag != types.SchemaTypeBodyStringType {
		t.Errorf("first element tag %d, want string", tag)
	}
	if tag := graph.TypeNodes[elems[1]].Body.Tag(); tag != types.SchemaTypeBodyS64Type {
		t.Errorf("second element tag %d, want s64", tag)
	}
}

func TestFlagsRoundTrip(t *testing.T) {
	assertRoundTrip(t, "flags", Perms{Read: true, Admin: true})
	assertRoundTrip(t, "flags none set", Perms{})
	assertRoundTrip(t, "flags all set", Perms{Read: true, Write: true, Admin: true})
}

// TestFlagsCarryTheirNames — the names come from the fields, lower-cased in
// declaration order, which is also the wire order of the bool vector.
func TestFlagsCarryTheirNames(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Perms]()))
	names := g.build().TypeNodes[root].Body.FlagsType()
	if !reflect.DeepEqual(names, []string{"read", "write", "admin"}) {
		t.Errorf("flag names %v, want [read write admin]", names)
	}
}

// TestDefineFlagsRejectsNonBoolFields — a flags set is bools only; a stray field
// would otherwise be silently dropped from the wire representation.
func TestDefineFlagsRejectsNonBoolFields(t *testing.T) {
	type Mixed struct {
		Read bool
		Name string
	}
	withDefs(t, func(d *definitions) {
		defineFlagsInto[Mixed](d)
		mustDefErr(t, d, "every flag field must be bool")
	})
}

func TestDefineFlagsRejectsNonStruct(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineFlagsInto[int32](d)
		mustDefErr(t, d, "DefineFlags requires a struct type")
	})
}

func TestDefineFlagsRejectsEmptyStruct(t *testing.T) {
	type Empty struct{}
	withDefs(t, func(d *definitions) {
		defineFlagsInto[Empty](d)
		mustDefErr(t, d, "needs at least one exported bool field")
	})
}
