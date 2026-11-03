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

package bridge

import (
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// throughTheWire encodes, sends the value through the REST wire form and back,
// and decodes, so a helper is checked against what actually travels.
func throughTheWire[T any](t *testing.T, name string, in T, enc Encoder[T], dec Decoder[T]) {
	t.Helper()
	data, err := schema.MarshalWireValue(enc(in))
	if err != nil {
		t.Fatalf("%s: marshal: %v", name, err)
	}
	sv, err := schema.UnmarshalWireValue(data)
	if err != nil {
		t.Fatalf("%s: unmarshal: %v", name, err)
	}
	out, err := dec(sv)
	if err != nil {
		t.Fatalf("%s: decode: %v", name, err)
	}
	if !reflect.DeepEqual(in, out) {
		t.Fatalf("%s: %#v came back as %#v", name, in, out)
	}
}

func TestLeavesRoundTrip(t *testing.T) {
	throughTheWire(t, "bool", true, EncodeBool, DecodeBool)
	throughTheWire(t, "s8", int8(-8), EncodeS8, DecodeS8)
	throughTheWire(t, "s64 extreme", int64(-9223372036854775808), EncodeS64, DecodeS64)
	throughTheWire(t, "u64 extreme", uint64(18446744073709551615), EncodeU64, DecodeU64)
	throughTheWire(t, "f32", float32(1.5), EncodeF32, DecodeF32)
	throughTheWire(t, "string", "héllo", EncodeString, DecodeString)
	throughTheWire(t, "char", values.Char('✓'), EncodeChar, DecodeChar)
	throughTheWire(t, "text", values.Text("prose"), EncodeText, DecodeText)
	throughTheWire(t, "binary", values.Binary{0, 127, 255}, EncodeBinary, DecodeBinary)
	throughTheWire(t, "empty binary", values.Binary{}, EncodeBinary, DecodeBinary)
	if out, err := DecodeBinary(schema.BinaryValue{}); err != nil || out == nil {
		t.Fatalf("an empty binary decoded to %#v, %v; want an empty, non-nil value", out, err)
	}
	inline := EncodeUnstructuredBinary(values.UnstructuredBinary[values.AnyMimeType]{})
	if out, err := DecodeUnstructuredBinary[values.AnyMimeType](inline); err != nil || out.Data == nil {
		t.Fatalf("empty inline data decoded to %#v, %v; want an empty, non-nil value", out, err)
	}
	throughTheWire(t, "path", values.Path("/tmp/x"), EncodePath, DecodePath)
	throughTheWire(t, "url", values.URL("https://example.test"), EncodeURL, DecodeURL)
	throughTheWire(t, "datetime",
		time.Date(2026, 9, 24, 12, 30, 0, 123456789, time.UTC), EncodeDatetime, DecodeDatetime)
	throughTheWire(t, "duration", -1500*time.Millisecond, EncodeDuration, DecodeDuration)
	throughTheWire(t, "uuid", values.UUID{0xdd, 0x00, 0x72, 0x1b, 0x33, 0x29, 0x46, 0x21, 0xa0, 0x1d, 0xc7, 0x1f, 0x02, 0xcd, 0x78, 0xc6}, EncodeUUID, DecodeUUID)
}

func TestCompositesRoundTrip(t *testing.T) {
	throughTheWire(t, "list", []int32{1, 2, 3},
		func(v []int32) schema.SchemaValue { return EncodeList(v, EncodeS32) },
		func(sv schema.SchemaValue) ([]int32, error) { return DecodeList(sv, DecodeS32) })

	throughTheWire(t, "empty list", []int32{},
		func(v []int32) schema.SchemaValue { return EncodeList(v, EncodeS32) },
		func(sv schema.SchemaValue) ([]int32, error) { return DecodeList(sv, DecodeS32) })

	throughTheWire(t, "map", map[string]bool{"a": true, "b": false},
		func(v map[string]bool) schema.SchemaValue { return EncodeMap(v, EncodeString, EncodeBool) },
		func(sv schema.SchemaValue) (map[string]bool, error) { return DecodeMap(sv, DecodeString, DecodeBool) })

	throughTheWire(t, "option some", values.Some("x"),
		func(v values.Option[string]) schema.SchemaValue { return EncodeOption(v, EncodeString) },
		func(sv schema.SchemaValue) (values.Option[string], error) { return DecodeOption(sv, DecodeString) })

	throughTheWire(t, "option none", values.None[string](),
		func(v values.Option[string]) schema.SchemaValue { return EncodeOption(v, EncodeString) },
		func(sv schema.SchemaValue) (values.Option[string], error) { return DecodeOption(sv, DecodeString) })

	type R = values.Result[uint64, string]
	enc := func(v R) schema.SchemaValue { return EncodeResult(v, EncodeU64, EncodeString) }
	dec := func(sv schema.SchemaValue) (R, error) { return DecodeResult(sv, DecodeU64, DecodeString) }
	throughTheWire(t, "result ok", values.Ok[uint64, string](7), enc, dec)
	throughTheWire(t, "result err", values.Err[uint64]("boom"), enc, dec)

	throughTheWire(t, "tuple", values.Tuple3[string, bool, int64]{A: "a", B: true, C: -1},
		func(v values.Tuple3[string, bool, int64]) schema.SchemaValue {
			return EncodeTuple3(v, EncodeString, EncodeBool, EncodeS64)
		},
		func(sv schema.SchemaValue) (values.Tuple3[string, bool, int64], error) {
			return DecodeTuple3(sv, DecodeString, DecodeBool, DecodeS64)
		})

	// Composites nest: a list of options.
	throughTheWire(t, "tuple1", values.Tuple1[bool]{A: true},
		func(v values.Tuple1[bool]) schema.SchemaValue { return EncodeTuple1(v, EncodeBool) },
		func(sv schema.SchemaValue) (values.Tuple1[bool], error) { return DecodeTuple1(sv, DecodeBool) })

	type node struct{ next *node }
	var encodeNode func(node) schema.SchemaValue
	encodeNode = func(n node) schema.SchemaValue {
		return schema.RecordValue{Fields: []schema.SchemaValue{EncodePointer(n.next, encodeNode)}}
	}
	var decodeNode func(schema.SchemaValue) (node, error)
	decodeNode = func(sv schema.SchemaValue) (node, error) {
		r, ok := sv.(schema.RecordValue)
		if !ok || len(r.Fields) != 1 {
			return node{}, Mismatch("record", sv)
		}
		next, err := DecodePointer(r.Fields[0], decodeNode)
		return node{next: next}, err
	}
	throughTheWire(t, "pointer", node{next: &node{next: &node{}}}, encodeNode, decodeNode)
	if sv := EncodePointer[node](nil, encodeNode); !reflect.DeepEqual(sv, schema.OptionValue{}) {
		t.Fatalf("a nil pointer encoded as %v", sv)
	}

	throughTheWire(t, "nested", []values.Option[int64]{values.Some[int64](1), values.None[int64]()},
		func(v []values.Option[int64]) schema.SchemaValue {
			return EncodeList(v, func(e values.Option[int64]) schema.SchemaValue { return EncodeOption(e, EncodeS64) })
		},
		func(sv schema.SchemaValue) ([]values.Option[int64], error) {
			return DecodeList(sv, func(e schema.SchemaValue) (values.Option[int64], error) { return DecodeOption(e, DecodeS64) })
		})
}

// A result arm that carries no value is spelled struct{} in Go and has no
// encoder; it travels without a value, and a value arriving for it is an error.
func TestAUnitResultArm(t *testing.T) {
	sv := EncodeResult(values.Ok[values.Unit, string](values.Unit{}), nil, EncodeString)
	if sv.(schema.ResultValue).Value != nil {
		t.Fatalf("a unit ok arm must travel without a value: %#v", sv)
	}
	got, err := DecodeResult[values.Unit, string](sv, nil, DecodeString)
	if err != nil || !got.IsOk() {
		t.Fatalf("decode: %#v %v", got, err)
	}

	withValue := schema.ResultValue{Value: ptr[schema.SchemaValue](schema.BoolValue{Value: true})}
	if _, err := DecodeResult[values.Unit, string](withValue, nil, DecodeString); err == nil ||
		!strings.Contains(err.Error(), "declares no value") {
		t.Fatalf("a value for a unit arm must be rejected, got %v", err)
	}
}

func TestDecodingTheWrongKindSaysWhatWasExpected(t *testing.T) {
	_, err := DecodeS32(schema.StringValue{Value: "x"})
	if err == nil || !strings.Contains(err.Error(), "expected a s32 value") {
		t.Fatalf("got %v", err)
	}
	_, err = DecodeFixedList(schema.FixedListValue{Items: []schema.SchemaValue{}}, 4, DecodeU8)
	if err == nil || !strings.Contains(err.Error(), "fixed list of 4") {
		t.Fatalf("got %v", err)
	}
	_, err = DecodeTuple2(schema.TupleValue{}, DecodeBool, DecodeBool)
	if err == nil || !strings.Contains(err.Error(), "2-tuple") {
		t.Fatalf("got %v", err)
	}
}

func ptr[T any](v T) *T { return &v }

// A key named twice keeps its later entry, as the host's validation and the
// Rust conversion do.
func TestAMapWithARepeatedKeyKeepsTheLaterEntry(t *testing.T) {
	m := schema.MapValue{Entries: []schema.MapEntry{
		{Key: schema.StringValue{Value: "k"}, Value: schema.BoolValue{Value: true}},
		{Key: schema.StringValue{Value: "k"}, Value: schema.BoolValue{Value: false}},
	}}
	got, err := DecodeMap(m, DecodeString, DecodeBool)
	if err != nil || len(got) != 1 || got["k"] {
		t.Fatalf("got %v, %v", got, err)
	}
}

func TestUnitIsTheEmptyTuple(t *testing.T) {
	sv := EncodeUnit(values.Unit{})
	if tv, ok := sv.(schema.TupleValue); !ok || len(tv.Elements) != 0 {
		t.Fatalf("unit encoded as %#v", sv)
	}
	data, err := schema.MarshalWireValue(sv)
	if err != nil {
		t.Fatal(err)
	}
	back, err := schema.UnmarshalWireValue(data)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := DecodeUnit(back); err != nil {
		t.Fatal(err)
	}
	if _, err := DecodeUnit(schema.TupleValue{Elements: []schema.SchemaValue{schema.BoolValue{}}}); err == nil {
		t.Fatal("a 1-tuple decoded as unit")
	}
}

// A string that is not valid UTF-8 is refused when the value is marshalled,
// rather than travelling with U+FFFD in its place.
func TestInvalidUTF8IsRefusedOnTheWire(t *testing.T) {
	for _, sv := range []schema.SchemaValue{
		EncodeString("ok\xffno"),
		EncodeText("ok\xffno"),
		EncodePath("ok\xffno"),
		EncodeURL("ok\xffno"),
		EncodeList([]string{"ok", "\xff"}, EncodeString),
	} {
		if _, err := schema.MarshalWireValue(sv); err == nil || !strings.Contains(err.Error(), "UTF-8") {
			t.Errorf("%#v: got %v", sv, err)
		}
	}
}

type meters struct{}

func (meters) BaseUnit() string          { return "m" }
func (meters) AllowedSuffixes() []string { return []string{"m", "km"} }

type kilograms struct{}

func (kilograms) BaseUnit() string          { return "kg" }
func (kilograms) AllowedSuffixes() []string { return nil }

// A quantity in a unit its schema does not list is an encode error, which
// Encode returns rather than letting it panic out of the client.
func TestEncodeRefusesAUnitTheQuantityDoesNotAccept(t *testing.T) {
	for _, accepted := range []func() schema.SchemaValue{
		func() schema.SchemaValue { return EncodeQuantity(values.Quantity[meters]{Mantissa: 1, Unit: "km"}) },
		func() schema.SchemaValue { return EncodeQuantity(values.Quantity[meters]{Mantissa: 1}) },
		func() schema.SchemaValue { return EncodeQuantity(values.Quantity[kilograms]{Mantissa: 1, Unit: "kg"}) },
	} {
		if _, err := Encode(accepted); err != nil {
			t.Fatalf("an accepted unit was refused: %v", err)
		}
	}
	for _, refused := range []func() schema.SchemaValue{
		func() schema.SchemaValue {
			return EncodeList([]values.Quantity[meters]{{Mantissa: 1, Unit: "mi"}}, EncodeQuantity[meters])
		},
		func() schema.SchemaValue { return EncodeQuantity(values.Quantity[kilograms]{Mantissa: 1, Unit: "g"}) },
	} {
		_, err := Encode(refused)
		var ee *EncodeError
		if !errors.As(err, &ee) || !strings.Contains(err.Error(), "unit") {
			t.Fatalf("got %v, want an *EncodeError naming the unit", err)
		}
	}
}

// Encode returns only an EncodeError: any other panic is a programming error
// and goes on.
func TestEncodeLetsOtherPanicsThrough(t *testing.T) {
	defer func() {
		if r := recover(); r != "boom" {
			t.Fatalf("recovered %v, want the original panic", r)
		}
	}()
	_, _ = Encode(func() schema.SchemaValue { panic("boom") })
}

// A configuration value its schema does not accept is reported by NewAgent.
func TestNewAgentReportsAConfigValueThatDoesNotEncode(t *testing.T) {
	configuration := Configuration{Server: Custom("http://localhost:1", "t"), AppName: "app", EnvName: "env"}
	_, err := NewAgent("scale", schema.RecordValue{}, WithConfiguration(configuration), WithTypedConfig(TypedConfigEntry{
		Path:  []string{"limit"},
		Type:  "{}",
		Value: func() schema.SchemaValue { return EncodeQuantity(values.Quantity[kilograms]{Mantissa: 1, Unit: "lb"}) },
	}))
	if err == nil || !strings.Contains(err.Error(), `unit "lb"`) {
		t.Fatalf("got %v", err)
	}
}
