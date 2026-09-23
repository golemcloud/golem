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
	"reflect"
	"strings"
	"testing"
)

func ptr[T any](v T) *T { return &v }

func value(v SchemaValue) *SchemaValue { return &v }

// everyValueCase is one example of each SchemaValue case, used both to pin the
// count and to round-trip the ones that travel.
func everyValueCase() []SchemaValue {
	return []SchemaValue{
		BoolValue{Value: true},
		S8Value{Value: -8},
		S16Value{Value: -16},
		S32Value{Value: -32},
		S64Value{Value: -9223372036854775808},
		U8Value{Value: 8},
		U16Value{Value: 16},
		U32Value{Value: 32},
		U64Value{Value: 18446744073709551615},
		F32Value{Value: 1.5},
		F64Value{Value: -2.25},
		CharValue{Value: '✓'},
		StringValue{Value: "hello"},
		RecordValue{Fields: []SchemaValue{BoolValue{Value: false}, StringValue{Value: "x"}}},
		VariantValue{Case: 2, Payload: value(StringValue{Value: "p"})},
		EnumValue{Case: 3},
		FlagsValue{Set: []bool{true, false, true}},
		TupleValue{Elements: []SchemaValue{S32Value{Value: 1}, StringValue{Value: "t"}}},
		ListValue{Items: []SchemaValue{U8Value{Value: 1}}},
		FixedListValue{Items: []SchemaValue{U8Value{Value: 2}}},
		MapValue{Entries: []MapEntry{{Key: StringValue{Value: "k"}, Value: S32Value{Value: 7}}}},
		OptionValue{Value: value(StringValue{Value: "some"})},
		ResultValue{IsErr: true, Value: value(StringValue{Value: "boom"})},
		TextValue{Text: "prose", Language: ptr("en")},
		BinaryValue{Bytes: []byte{0, 127, 255}, MimeType: ptr("application/octet-stream")},
		PathValue{Value: "/tmp/x"},
		UrlValue{Value: "https://example.test/a"},
		DatetimeValue{Seconds: 1700000000, Nanoseconds: 123456789},
		DurationValue{Nanoseconds: -9007199254740993},
		QuantityValueNode{Value: QuantityValue{Mantissa: 12345, Scale: 3, Unit: "kg"}},
		UnionValue{Tag: "inline", Body: StringValue{Value: "u"}},
		SecretValue{},
		QuotaTokenValue{},
		PermissionCardValue{},
		StreamValue{},
	}
}

// Go switches are not exhaustive, so a SchemaValue case added to the model
// without a case in the wire codec would only fail at runtime. Pinning the
// count turns that into a failing test.
func TestEveryValueCaseIsAccountedFor(t *testing.T) {
	cases := everyValueCase()
	if len(cases) != wireValueKinds {
		t.Fatalf("the wire codec knows %d value kinds, the test lists %d", wireValueKinds, len(cases))
	}
	seen := map[reflect.Type]bool{}
	for _, c := range cases {
		typ := reflect.TypeOf(c)
		if seen[typ] {
			t.Fatalf("%v is listed twice", typ)
		}
		seen[typ] = true
		if _, err := valueToWire(c); err != nil && !strings.Contains(err.Error(), "the host holds") {
			t.Fatalf("%v: %v", typ, err)
		}
	}
}

func TestValuesRoundTripThroughTheWireForm(t *testing.T) {
	for _, original := range everyValueCase() {
		switch original.(type) {
		case SecretValue, QuotaTokenValue, PermissionCardValue, StreamValue:
			continue
		}
		data, err := MarshalWireValue(original)
		if err != nil {
			t.Fatalf("%T: marshal: %v", original, err)
		}
		back, err := UnmarshalWireValue(data)
		if err != nil {
			t.Fatalf("%T: unmarshal %s: %v", original, data, err)
		}
		if !reflect.DeepEqual(original, back) {
			t.Fatalf("%T: %#v round-tripped to %#v via %s", original, original, back, data)
		}
	}
}

// The wire form is structural and tagged; it is not canonical JSON. These are
// the shapes the server's serde derive produces, spelled out so a change to
// either side shows up here rather than in production.
func TestWireShapesMatchTheServer(t *testing.T) {
	cases := []struct {
		name  string
		value SchemaValue
		want  string
	}{
		{"s64 is a number, not a canonical string", S64Value{Value: -5}, `{"kind":"s64","value":-5}`},
		{"u64 keeps its full range", U64Value{Value: 18446744073709551615},
			`{"kind":"u64","value":18446744073709551615}`},
		{"a record is positional", RecordValue{Fields: []SchemaValue{BoolValue{Value: true}}},
			`{"kind":"record","value":{"fields":[{"kind":"bool","value":true}]}}`},
		{"an absent option omits inner", OptionValue{},
			`{"kind":"option","value":{}}`},
		{"a unit ok omits value", ResultValue{},
			`{"kind":"result","value":{"tag":"ok"}}`},
		{"flags are positional bits", FlagsValue{Set: []bool{true, false}},
			`{"kind":"flags","value":{"bits":[true,false]}}`},
		{"a map is a list of pairs", MapValue{Entries: []MapEntry{{
			Key: StringValue{Value: "k"}, Value: BoolValue{Value: false}}}},
			`{"kind":"map","value":{"entries":[[{"kind":"string","value":"k"},` +
				`{"kind":"bool","value":false}]]}}`},
		{"binary is a byte array, not base64",
			BinaryValue{Bytes: []byte{1, 2}},
			`{"kind":"binary","value":{"bytes":[1,2]}}`},
		{"binary names its mime type in camel case",
			BinaryValue{Bytes: []byte{}, MimeType: ptr("image/png")},
			`{"kind":"binary","value":{"bytes":[],"mimeType":"image/png"}}`},
		{"a duration is a number of nanoseconds", DurationValue{Nanoseconds: 1500},
			`{"kind":"duration","value":{"nanoseconds":1500}}`},
		{"a quantity carries its parts as numbers",
			QuantityValueNode{Value: QuantityValue{Mantissa: 5, Scale: 2, Unit: "m"}},
			`{"kind":"quantity","value":{"mantissa":5,"scale":2,"unit":"m"}}`},
		{"a char is a one-character string", CharValue{Value: 'ß'},
			`{"kind":"char","value":"ß"}`},
		{"a datetime is RFC 3339 in UTC",
			DatetimeValue{Seconds: 0, Nanoseconds: 0},
			`{"kind":"datetime","value":{"value":"1970-01-01T00:00:00Z"}}`},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			data, err := MarshalWireValue(c.value)
			if err != nil {
				t.Fatalf("marshal: %v", err)
			}
			if got := canonicalize(t, data); got != canonicalize(t, []byte(c.want)) {
				t.Fatalf("got %s, want %s", got, c.want)
			}
		})
	}
}

// canonicalize re-encodes through a map so field order does not decide whether
// the test passes: only the content does.
func canonicalize(t *testing.T, data []byte) string {
	t.Helper()
	var v any
	if err := json.Unmarshal(data, &v); err != nil {
		t.Fatalf("not JSON: %s: %v", data, err)
	}
	out, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("re-encode: %v", err)
	}
	return string(out)
}

func TestAnExplicitNullReadsAsAnAbsentPayload(t *testing.T) {
	// The server's serde output renders an empty optional payload as null,
	// while other SDKs omit the field. Both must mean the same thing.
	for _, data := range []string{
		`{"kind":"option","value":{"inner":null}}`,
		`{"kind":"option","value":{}}`,
	} {
		v, err := UnmarshalWireValue([]byte(data))
		if err != nil {
			t.Fatalf("%s: %v", data, err)
		}
		if opt, ok := v.(OptionValue); !ok || opt.Value != nil {
			t.Fatalf("%s decoded to %#v, want an absent option", data, v)
		}
	}
}

func TestHostHandlesAreRejectedInBothDirections(t *testing.T) {
	if _, err := MarshalWireValue(SecretValue{}); err == nil ||
		!strings.Contains(err.Error(), "the host holds") {
		t.Fatalf("marshalling a secret should say why it cannot travel, got %v", err)
	}
	if _, err := UnmarshalWireValue([]byte(`{"kind":"stream","value":{}}`)); err == nil ||
		!strings.Contains(err.Error(), "the host holds") {
		t.Fatalf("reading a stream should say why it cannot be read, got %v", err)
	}
}

func TestMalformedValuesAreRejected(t *testing.T) {
	cases := map[string]string{
		`{"kind":"nope","value":1}`:                     "unsupported schema value kind",
		`{"kind":"char","value":"ab"}`:                  "one Unicode scalar value",
		`{"kind":"result","value":{"tag":"maybe"}}`:     "result tag must be ok or err",
		`{"kind":"map","value":{"entries":[[]]}}`:       "[key, value] pair",
		`{"kind":"datetime","value":{"value":"today"}}`: "datetime",
		`{"kind":"s8","value":"not a number"}`:          "s8 value",
	}
	for data, want := range cases {
		_, err := UnmarshalWireValue([]byte(data))
		if err == nil || !strings.Contains(err.Error(), want) {
			t.Fatalf("%s: got %v, want an error mentioning %q", data, err, want)
		}
	}
}
