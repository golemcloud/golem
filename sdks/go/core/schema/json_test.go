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
	"errors"
	"reflect"
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

// TestWideIntegersAreCanonicalStrings: s64 and u64 do not fit a JSON number,
// so they travel as canonical base-10 strings, never as numbers capped at
// 2^53-1.
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

// TestWideIntegerAsNumberIsRejected — a wide integer as a JSON number loses
// precision past 2^53, so it is not accepted.
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

// TestDurationIsAnObjectOfNanoseconds — not an ISO-8601 string.
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

func fieldDefaultsRef() Ref {
	root := typ(RecordType{Fields: []NamedField{
		{Name: "name", Body: typ(StringType{})},
		{Name: "retries", Body: typ(U32Type{})},
		{Name: "nickname", Body: typ(OptionType{Inner: typ(StringType{})})},
		{Name: "label", Body: typ(OptionType{Inner: typ(StringType{})})},
		{Name: "inner", Body: typ(RecordType{Fields: []NamedField{{Name: "retries", Body: typ(U32Type{})}}})},
	}})
	x := SchemaValue(StringValue{Value: "x"})
	return NewRef(SchemaGraph{Root: root}).WithFieldDefaults(map[string]SchemaValue{
		"retries": U32Value{Value: 3},
		"label":   OptionValue{Value: &x},
	})
}

// TestFieldDefaultsFillOmittedRootFields — a root field with a default may be
// left out of canonical JSON and packs as that default, while an option field
// without one is still none and an explicit value, null included, wins.
func TestFieldDefaultsFillOmittedRootFields(t *testing.T) {
	r := fieldDefaultsRef()
	got, err := r.PackJSONBytes([]byte(`{"name":"a","inner":{"retries":1}}`))
	if err != nil {
		t.Fatalf("PackJSONBytes: %v", err)
	}
	x := SchemaValue(StringValue{Value: "x"})
	want := RecordValue{Fields: []SchemaValue{
		StringValue{Value: "a"}, U32Value{Value: 3}, OptionValue{}, OptionValue{Value: &x},
		RecordValue{Fields: []SchemaValue{U32Value{Value: 1}}},
	}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("packed %#v, want %#v", got, want)
	}
	if err := r.Validate(got); err != nil {
		t.Errorf("the filled record does not validate: %v", err)
	}

	got, err = r.PackJSONBytes([]byte(`{"name":"a","retries":5,"label":null,"inner":{"retries":1}}`))
	if err != nil {
		t.Fatalf("PackJSONBytes: %v", err)
	}
	fields := got.(RecordValue).Fields
	if fields[1] != (U32Value{Value: 5}) || !reflect.DeepEqual(fields[3], OptionValue{}) {
		t.Errorf("explicit values packed as %#v", fields)
	}
}

// TestFieldDefaultsApplyOnlyToTheRootRecord — a nested field that shares a
// defaulted root field's name, an unknown field and a missing required field
// are reported as before, each at its own path.
func TestFieldDefaultsApplyOnlyToTheRootRecord(t *testing.T) {
	r := fieldDefaultsRef()
	_, err := r.PackJSONBytes([]byte(`{"inner":{},"extra":1}`))
	var ve *ValidationError
	if !errors.As(err, &ve) {
		t.Fatalf("got %v, want a validation error", err)
	}
	var paths []string
	for _, i := range ve.Issues {
		paths = append(paths, i.Path+": "+i.Message)
	}
	want := []string{`extra: unknown field "extra"`, `name: missing field "name"`, `inner.retries: missing field "retries"`}
	if !reflect.DeepEqual(paths, want) {
		t.Errorf("issues %q, want %q", paths, want)
	}
	if _, err := r.At(r.Type()).PackJSONBytes([]byte(`{"name":"a","inner":{"retries":1}}`)); err == nil ||
		!strings.Contains(err.Error(), `missing field "retries"`) {
		t.Errorf("At kept the field defaults: %v", err)
	}
}

// TestFieldDefaultsDoNotShareStorage — mutating a packed default, or the value
// handed to WithFieldDefaults, does not change what a later pack produces.
func TestFieldDefaultsDoNotShareStorage(t *testing.T) {
	root := typ(RecordType{Fields: []NamedField{
		{Name: "tags", Body: typ(ListType{Element: typ(StringType{})})},
		{Name: "payload", Body: typ(BinaryType{})},
		{Name: "label", Body: typ(OptionType{Inner: typ(StringType{})})},
	}})
	label := SchemaValue(StringValue{Value: "x"})
	tags := ListValue{Items: []SchemaValue{StringValue{Value: "a"}}}
	payload := BinaryValue{Bytes: []byte{1, 2}}
	r := NewRef(SchemaGraph{Root: root}).WithFieldDefaults(map[string]SchemaValue{
		"tags": tags, "payload": payload, "label": OptionValue{Value: &label},
	})
	x := SchemaValue(StringValue{Value: "x"})
	want := RecordValue{Fields: []SchemaValue{
		ListValue{Items: []SchemaValue{StringValue{Value: "a"}}},
		BinaryValue{Bytes: []byte{1, 2}},
		OptionValue{Value: &x},
	}}

	tags.Items[0] = StringValue{Value: "changed"}
	payload.Bytes[0] = 9
	label = StringValue{Value: "changed"}
	first, err := r.PackJSON(map[string]any{})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	if !reflect.DeepEqual(first, want) {
		t.Fatalf("mutating the declared defaults changed the pack: %#v", first)
	}

	fields := first.(RecordValue).Fields
	fields[0].(ListValue).Items[0] = StringValue{Value: "mutated"}
	fields[1].(BinaryValue).Bytes[0] = 9
	*fields[2].(OptionValue).Value = StringValue{Value: "mutated"}
	second, err := r.PackJSON(map[string]any{})
	if err != nil {
		t.Fatalf("PackJSON: %v", err)
	}
	if !reflect.DeepEqual(second, want) {
		t.Errorf("mutating a packed value changed the next pack: %#v", second)
	}
}

// TestFieldDefaultIsCheckedAgainstItsField — a default that does not fit its
// field is a problem at that field, not a value sent on.
func TestFieldDefaultIsCheckedAgainstItsField(t *testing.T) {
	root := typ(RecordType{Fields: []NamedField{{Name: "retries", Body: typ(U32Type{})}}})
	r := NewRef(SchemaGraph{Root: root}).WithFieldDefaults(map[string]SchemaValue{"retries": StringValue{Value: "3"}})
	_, err := r.PackJSON(map[string]any{})
	var ve *ValidationError
	if !errors.As(err, &ve) || len(ve.Issues) != 1 || ve.Issues[0].Path != "retries" {
		t.Errorf("got %v, want one issue at retries", err)
	}
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

func TestUUIDIsAHyphenatedString(t *testing.T) {
	in := `"dd00721b-3329-4621-a01d-c71f02cd78c6"`
	if got := roundTrip(t, typ(UUIDType{}), in); got != in {
		t.Errorf("round trip gave %s", got)
	}
	if got := roundTrip(t, typ(UUIDType{}), `"DD00721B-3329-4621-A01D-C71F02CD78C6"`); got != in {
		t.Errorf("an upper-case UUID round-tripped to %s", got)
	}
	mustReject(t, typ(UUIDType{}), `"dd00721b33294621a01dc71f02cd78c6"`, "hyphenated UUID")
	mustReject(t, typ(UUIDType{}), `42`, "hyphenated UUID string")
}

// A float beyond the int64 range must not convert into a value that passes the
// range check, an f32 that overflows is not infinity, and nothing may follow
// the value.
func TestNumbersOutOfRangeAndTrailingDataAreRejected(t *testing.T) {
	ref := NewRef(SchemaGraph{Root: typ(S32Type{})})
	for _, big := range []float64{1e20, -1e20, 0x1p63} {
		if _, err := ref.PackJSON(big); err == nil || !strings.Contains(err.Error(), "outside the range") {
			t.Errorf("packing %v as s32: %v", big, err)
		}
	}
	mustReject(t, typ(F32Type{}), `1e300`, "outside the f32 range")
	mustReject(t, typ(F32Type{}), `-1e39`, "outside the f32 range")
	if got := roundTrip(t, typ(F32Type{}), `1.5`); got != `1.5` {
		t.Errorf("f32 round-tripped to %s", got)
	}
	for _, in := range []string{`1 2`, `{"a":1} garbage`, `"x"]`} {
		if _, err := NewRef(SchemaGraph{Root: typ(StringType{})}).PackJSONBytes([]byte(in)); err == nil {
			t.Errorf("accepted %q", in)
		}
	}
}

// A string that is not valid UTF-8 is refused, rather than rendered with
// U+FFFD in place of each invalid byte, on both the canonical and the wire
// path.
func TestInvalidUTF8IsRefused(t *testing.T) {
	bad := "ok\xffno"
	for _, c := range []struct {
		typ   SchemaType
		value SchemaValue
	}{
		{typ(StringType{}), StringValue{Value: bad}},
		{typ(TextType{}), TextValue{Text: bad}},
		{typ(PathType{}), PathValue{Value: bad}},
		{typ(UrlType{}), UrlValue{Value: bad}},
	} {
		if _, err := NewRef(SchemaGraph{Root: c.typ}).UnpackJSON(c.value); err == nil || !strings.Contains(err.Error(), "UTF-8") {
			t.Errorf("UnpackJSON(%#v): %v", c.value, err)
		}
		if _, err := MarshalWireValue(c.value); err == nil || !strings.Contains(err.Error(), "UTF-8") {
			t.Errorf("MarshalWireValue(%#v): %v", c.value, err)
		}
	}
	lang := bad
	if _, err := MarshalWireValue(TextValue{Text: "ok", Language: &lang}); err == nil {
		t.Error("a text language that is not UTF-8 was marshalled")
	}
}
