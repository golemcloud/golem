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

// roundTrip packs canonical JSON and renders it back, which is the property
// every case below turns on.
func roundTrip(t *testing.T, root SchemaType, in string) string {
	t.Helper()
	r := NewRef(SchemaGraph{Root: root})
	v, err := r.PackJSONBytes([]byte(in))
	if err != nil {
		t.Fatalf("PackJSONBytes(%s): %v", in, err)
	}
	out, err := r.UnpackJSONBytes(v)
	if err != nil {
		t.Fatalf("UnpackJSONBytes: %v", err)
	}
	return string(out)
}

func mustReject(t *testing.T, root SchemaType, in, wantMsg string) {
	t.Helper()
	_, err := NewRef(SchemaGraph{Root: root}).PackJSONBytes([]byte(in))
	if err == nil {
		t.Fatalf("packing %s succeeded, want a failure mentioning %q", in, wantMsg)
	}
	if !strings.Contains(err.Error(), wantMsg) {
		t.Errorf("packing %s failed with %q, want it to mention %q", in, err, wantMsg)
	}
}

// TestWideIntegersAreCanonicalStrings is the GOL-653 property: s64 and u64 do
// not fit a JSON number, so they travel as canonical base-10 strings. MoonBit
// and Scala render them as numbers and cap them at 2^53-1; Go must not.
func TestWideIntegersAreCanonicalStrings(t *testing.T) {
	if got := roundTrip(t, typ(S64Type{}), `"-9223372036854775808"`); got != `"-9223372036854775808"` {
		t.Errorf("s64 round-tripped to %s", got)
	}
	if got := roundTrip(t, typ(U64Type{}), `"18446744073709551615"`); got != `"18446744073709551615"` {
		t.Errorf("u64 round-tripped to %s", got)
	}
	// Values far beyond the double-safe range survive exactly.
	if got := roundTrip(t, typ(S64Type{}), `"9007199254740993"`); got != `"9007199254740993"` {
		t.Errorf("a value past 2^53 round-tripped to %s", got)
	}
}

// TestWideIntegerAsNumberIsRejected — accepting a number here is precisely the
// divergence GOL-653 records.
func TestWideIntegerAsNumberIsRejected(t *testing.T) {
	mustReject(t, typ(S64Type{}), `42`, "canonical integer string")
	mustReject(t, typ(U64Type{}), `42`, "canonical integer string")
}

// TestNonCanonicalIntegerStringsAreRejected — one value must have one
// spelling, because receivers compare these as strings.
func TestNonCanonicalIntegerStringsAreRejected(t *testing.T) {
	for _, bad := range []string{`"007"`, `"+7"`, `"-0"`, `" 7"`, `"7 "`, `"0x7"`, `""`} {
		mustReject(t, typ(S64Type{}), bad, "canonical base-10 integer string")
	}
}

// TestNarrowIntegersAreNumbers — the other half of the rule: through 32 bits
// they are JSON numbers, not strings.
func TestNarrowIntegersAreNumbers(t *testing.T) {
	for _, tc := range []struct {
		root SchemaType
		in   string
	}{
		{typ(S8Type{}), `-128`},
		{typ(S16Type{}), `32767`},
		{typ(S32Type{}), `-2147483648`},
		{typ(U8Type{}), `255`},
		{typ(U32Type{}), `4294967295`},
	} {
		if got := roundTrip(t, tc.root, tc.in); got != tc.in {
			t.Errorf("%s round-tripped to %s", tc.in, got)
		}
	}
	mustReject(t, typ(S8Type{}), `128`, "outside the range")
	mustReject(t, typ(S32Type{}), `"5"`, "expected integer")
}

// TestDurationIsAnObjectOfNanoseconds — not an ISO-8601 string, which is the
// other half of the GOL-653 divergence.
func TestDurationIsAnObjectOfNanoseconds(t *testing.T) {
	in := `{"nanoseconds":"-1500000000"}`
	if got := roundTrip(t, typ(DurationType{}), in); got != in {
		t.Errorf("duration round-tripped to %s", got)
	}
	mustReject(t, typ(DurationType{}), `"PT1.5S"`, "nanoseconds")
	mustReject(t, typ(DurationType{}), `{"nanoseconds":1500000000}`, "canonical integer string")
}

func TestQuantityCarriesMantissaScaleAndUnit(t *testing.T) {
	in := `{"mantissa":"12345","scale":3,"unit":"kg"}`
	if got := roundTrip(t, typ(QuantityType{Spec: QuantitySpec{BaseUnit: "kg"}}), in); got != in {
		t.Errorf("quantity round-tripped to %s", got)
	}
	mustReject(t, typ(QuantityType{Spec: QuantitySpec{BaseUnit: "kg"}}),
		`{"mantissa":12345,"scale":3,"unit":"kg"}`, "canonical integer string")
}

func TestBinaryIsBase64URLWithoutPadding(t *testing.T) {
	// 0xFB 0xFF encodes to "-_8" in base64url; the standard alphabet would use
	// "+/" and padding, so this also pins the alphabet.
	in := `{"bytes":"-_8"}`
	if got := roundTrip(t, typ(BinaryType{}), in); got != in {
		t.Errorf("binary round-tripped to %s", got)
	}
	mustReject(t, typ(BinaryType{}), `{"bytes":"+/8="}`, "base64url")
}

func TestOptionAbsentIsNull(t *testing.T) {
	root := typ(OptionType{Inner: typ(StringType{})})
	if got := roundTrip(t, root, `null`); got != `null` {
		t.Errorf("an absent option round-tripped to %s", got)
	}
	if got := roundTrip(t, root, `"here"`); got != `"here"` {
		t.Errorf("a present option round-tripped to %s", got)
	}
}

func TestResultArms(t *testing.T) {
	root := typ(ResultType{
		Ok:  &SchemaType{Body: S32Type{}},
		Err: &SchemaType{Body: StringType{}},
	})
	for _, in := range []string{`{"ok":7}`, `{"err":"nope"}`} {
		if got := roundTrip(t, root, in); got != in {
			t.Errorf("%s round-tripped to %s", in, got)
		}
	}
	// An arm that declares no payload carries null.
	bare := typ(ResultType{Err: &SchemaType{Body: StringType{}}})
	if got := roundTrip(t, bare, `{"ok":null}`); got != `{"ok":null}` {
		t.Errorf("a payload-less ok arm round-tripped to %s", got)
	}
}

// TestVariantPayloadShape — a case with no payload is its bare name; one with
// a payload is a single-key object.
func TestVariantPayloadShape(t *testing.T) {
	root := typ(VariantType{Cases: []VariantCase{
		{Name: "none"},
		{Name: "some", Payload: &SchemaType{Body: S32Type{}}},
	}})
	if got := roundTrip(t, root, `"none"`); got != `"none"` {
		t.Errorf("a payload-less case round-tripped to %s", got)
	}
	if got := roundTrip(t, root, `{"some":5}`); got != `{"some":5}` {
		t.Errorf("a payload case round-tripped to %s", got)
	}
	mustReject(t, root, `{"none":5}`, "carries no payload")
	mustReject(t, root, `"some"`, "needs a payload")
	mustReject(t, root, `"absent"`, "not a declared case")
}

// TestFlagsAreTheSelectedNames — the wire form is the set of names that are
// on, not a record of booleans.
func TestFlagsAreTheSelectedNames(t *testing.T) {
	root := typ(FlagsType{Flags: []string{"read", "write", "admin"}})
	if got := roundTrip(t, root, `["read","admin"]`); got != `["read","admin"]` {
		t.Errorf("flags round-tripped to %s", got)
	}
	if got := roundTrip(t, root, `[]`); got != `[]` {
		t.Errorf("an empty flag set round-tripped to %s", got)
	}
	mustReject(t, root, `["nope"]`, "not a declared flag")
}

// TestMapIsAnArrayOfPairs — keys need not be strings, so a map cannot be a
// JSON object.
func TestMapIsAnArrayOfPairs(t *testing.T) {
	root := typ(MapType{Key: typ(S32Type{}), Value: typ(StringType{})})
	in := `[[1,"one"],[2,"two"]]`
	if got := roundTrip(t, root, in); got != in {
		t.Errorf("map round-tripped to %s", got)
	}
	mustReject(t, root, `{"1":"one"}`, "array of [key, value] pairs")
}

func TestRecordRejectsUnknownAndMissingFields(t *testing.T) {
	root := typ(RecordType{Fields: []NamedField{
		{Name: "name", Body: typ(StringType{})},
		{Name: "count", Body: typ(S32Type{})},
	}})
	if got := roundTrip(t, root, `{"count":2,"name":"a"}`); got != `{"count":2,"name":"a"}` {
		t.Errorf("record round-tripped to %s", got)
	}
	mustReject(t, root, `{"name":"a"}`, `missing field "count"`)
	mustReject(t, root, `{"name":"a","count":2,"extra":true}`, `unknown field "extra"`)
}

// TestUnionPicksTheFirstBranchThatFits — the tag is not in the JSON, so
// branches are tried in declaration order.
func TestUnionPicksTheFirstBranchThatFits(t *testing.T) {
	root := typ(UnionType{Branches: []UnionBranch{
		{Tag: "count", Body: typ(S32Type{}), Discriminator: RegexRule{Pattern: `^\d+$`}},
		{Tag: "label", Body: typ(StringType{}), Discriminator: PrefixRule{Value: ""}},
	}})
	if got := roundTrip(t, root, `7`); got != `7` {
		t.Errorf("a numeric union value round-tripped to %s", got)
	}
	if got := roundTrip(t, root, `"seven"`); got != `"seven"` {
		t.Errorf("a string union value round-tripped to %s", got)
	}
	mustReject(t, root, `true`, "matches no declared union branch")
}

func TestDatetimeIsRFC3339(t *testing.T) {
	in := `"2026-09-23T12:34:56.789Z"`
	if got := roundTrip(t, typ(DatetimeType{}), in); got != in {
		t.Errorf("datetime round-tripped to %s", got)
	}
	mustReject(t, typ(DatetimeType{}), `"yesterday"`, "RFC 3339")
}

func TestCharIsOneCodePoint(t *testing.T) {
	if got := roundTrip(t, typ(CharType{}), `"é"`); got != `"é"` {
		t.Errorf("char round-tripped to %s", got)
	}
	mustReject(t, typ(CharType{}), `"ab"`, "exactly one character")
}

// TestHostManagedValuesHaveNoJSON — a caller never sees the material behind a
// capability, so there is nothing to render or build.
func TestHostManagedValuesHaveNoJSON(t *testing.T) {
	for _, root := range []SchemaType{
		typ(SecretType{Inner: typ(StringType{})}),
		typ(QuotaTokenType{}),
		typ(PermissionCardType{}),
	} {
		mustReject(t, root, `"anything"`, "host-managed capabilities")
	}
	mustReject(t, typ(StreamType{Item: &SchemaType{Body: StringType{}}}), `[]`,
		"streams and futures")
}

// TestNestedCompositesRoundTrip — the shapes compose, and a path in an error
// names where the problem is.
func TestNestedCompositesRoundTrip(t *testing.T) {
	root := typ(RecordType{Fields: []NamedField{
		{Name: "id", Body: typ(S64Type{})},
		{Name: "tags", Body: typ(ListType{Element: typ(StringType{})})},
		{Name: "size", Body: typ(OptionType{Inner: typ(QuantityType{Spec: QuantitySpec{BaseUnit: "B"}})})},
	}})
	in := `{"id":"1","size":{"mantissa":"5","scale":0,"unit":"B"},"tags":["a","b"]}`
	if got := roundTrip(t, root, in); got != in {
		t.Errorf("round-tripped to %s", got)
	}
	mustReject(t, root, `{"id":"1","tags":["a",2],"size":null}`, "tags[1]")
}

// TestPackedValuesAreOrdinaryGoValues — a caller can inspect what came back
// without reaching for reflection.
func TestPackedValuesAreOrdinaryGoValues(t *testing.T) {
	r := NewRef(SchemaGraph{Root: typ(RecordType{Fields: []NamedField{
		{Name: "n", Body: typ(S32Type{})},
	}})})
	v, err := r.PackJSONBytes([]byte(`{"n":3}`))
	if err != nil {
		t.Fatalf("PackJSONBytes: %v", err)
	}
	rec, ok := v.(RecordValue)
	if !ok {
		t.Fatalf("packed to %T, want RecordValue", v)
	}
	n, ok := rec.Fields[0].(S32Value)
	if !ok || n.Value != 3 {
		t.Errorf("field is %#v", rec.Fields[0])
	}
}

var _ = json.Marshal
