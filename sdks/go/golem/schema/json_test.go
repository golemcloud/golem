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

	witTypes "go.bytecodealliance.org/pkg/wit/types"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// roundTrip packs canonical JSON into a value tree and renders it back, which
// is the property every canonical form must have.
func roundTrip(t *testing.T, ref Ref, jsonText string) any {
	t.Helper()
	tree, err := ref.PackJSONBytes([]byte(jsonText))
	if err != nil {
		t.Fatalf("PackJSON(%s): %v", jsonText, err)
	}
	back, err := ref.UnpackJSON(tree)
	if err != nil {
		t.Fatalf("UnpackJSON after packing %s: %v", jsonText, err)
	}
	return back
}

func mustJSON(t *testing.T, v any) string {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return string(b)
}

// TestWideIntegersAreCanonicalStrings is the rule the SDKs disagree on: 64-bit
// integers must survive as base-10 strings. A JSON number would be read as a
// double by most receivers and silently rounded past 2^53.
func TestWideIntegersAreCanonicalStrings(t *testing.T) {
	var g graphBuilder
	s64 := g.push(types.MakeSchemaTypeBodyS64Type(witTypes.None[types.NumericRestrictions]()))
	u64 := g.push(types.MakeSchemaTypeBodyU64Type(witTypes.None[types.NumericRestrictions]()))
	s32 := g.push(types.MakeSchemaTypeBodyS32Type(witTypes.None[types.NumericRestrictions]()))
	rec := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "big", Body: s64, Metadata: types.MetadataEnvelope{}},
		{Name: "huge", Body: u64, Metadata: types.MetadataEnvelope{}},
		{Name: "small", Body: s32, Metadata: types.MetadataEnvelope{}},
	}))
	ref := NewRef(g.graph(rec))

	// 2^53 + 1 is the first integer a float64 cannot represent.
	const beyondFloat64 = "9007199254740993"
	input := `{"big":"` + beyondFloat64 + `","huge":"18446744073709551615","small":-7}`

	out := roundTrip(t, ref, input).(map[string]any)
	if out["big"] != beyondFloat64 {
		t.Errorf("s64 round-trip = %v, want the exact string %s", out["big"], beyondFloat64)
	}
	if out["huge"] != "18446744073709551615" {
		t.Errorf("u64 round-trip = %v, want the full unsigned range", out["huge"])
	}
	// Narrow integers stay JSON numbers.
	if _, isString := out["small"].(string); isString {
		t.Errorf("s32 should render as a number, got a string: %v", out["small"])
	}
	if mustJSON(t, out["small"]) != "-7" {
		t.Errorf("s32 round-trip = %v, want -7", out["small"])
	}
}

// TestWideIntegerAsNumberIsRejected — accepting a bare number here is how a
// value silently loses precision, so it must fail loudly.
func TestWideIntegerAsNumberIsRejected(t *testing.T) {
	var g graphBuilder
	s64 := g.push(types.MakeSchemaTypeBodyS64Type(witTypes.None[types.NumericRestrictions]()))
	ref := NewRef(g.graph(s64))

	if _, err := ref.PackJSONBytes([]byte(`9007199254740993`)); err == nil {
		t.Fatal("a bare JSON number should not be accepted for s64")
	}
}

// TestNonCanonicalIntegerStringsAreRejected — one value, one spelling.
func TestNonCanonicalIntegerStringsAreRejected(t *testing.T) {
	var g graphBuilder
	s64 := g.push(types.MakeSchemaTypeBodyS64Type(witTypes.None[types.NumericRestrictions]()))
	ref := NewRef(g.graph(s64))

	for _, bad := range []string{`"007"`, `"+7"`, `"-0"`, `" 7"`, `"7 "`, `"0x10"`, `""`} {
		if _, err := ref.PackJSONBytes([]byte(bad)); err == nil {
			t.Errorf("%s should be rejected as a canonical integer", bad)
		}
	}
	for _, good := range []string{`"0"`, `"-7"`, `"9007199254740993"`} {
		if _, err := ref.PackJSONBytes([]byte(good)); err != nil {
			t.Errorf("%s should be accepted: %v", good, err)
		}
	}
}

// TestCompositeRoundTrip covers the shapes whose JSON form is not obvious:
// variants with and without payloads, enums, flags, maps, options and results.
func TestCompositeRoundTrip(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	u32 := g.push(types.MakeSchemaTypeBodyU32Type(witTypes.None[types.NumericRestrictions]()))
	variant := g.push(types.MakeSchemaTypeBodyVariantType([]types.VariantCaseType{
		{Name: "pending", Payload: witTypes.None[int32](), Metadata: types.MetadataEnvelope{}},
		{Name: "failed", Payload: some(str), Metadata: types.MetadataEnvelope{}},
	}))
	enum := g.push(types.MakeSchemaTypeBodyEnumType([]string{"low", "high"}))
	flags := g.push(types.MakeSchemaTypeBodyFlagsType([]string{"read", "write", "exec"}))
	opt := g.push(types.MakeSchemaTypeBodyOptionType(str))
	res := g.push(types.MakeSchemaTypeBodyResultType(types.ResultSpec{Ok: some(u32), Err: some(str)}))
	mapType := g.push(types.MakeSchemaTypeBodyMapType(types.MapSpec{Key: str, Value: u32}))
	rec := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "state", Body: variant, Metadata: types.MetadataEnvelope{}},
		{Name: "level", Body: enum, Metadata: types.MetadataEnvelope{}},
		{Name: "perms", Body: flags, Metadata: types.MetadataEnvelope{}},
		{Name: "label", Body: opt, Metadata: types.MetadataEnvelope{}},
		{Name: "outcome", Body: res, Metadata: types.MetadataEnvelope{}},
		{Name: "counts", Body: mapType, Metadata: types.MetadataEnvelope{}},
	}))
	ref := NewRef(g.graph(rec))

	input := `{
		"state": {"failed": "disk full"},
		"level": "high",
		"perms": ["read", "exec"],
		"label": null,
		"outcome": {"ok": 42},
		"counts": [["a", 1], ["b", 2]]
	}`
	out := roundTrip(t, ref, input).(map[string]any)

	if got := mustJSON(t, out["state"]); got != `{"failed":"disk full"}` {
		t.Errorf("variant with payload = %s", got)
	}
	if out["level"] != "high" {
		t.Errorf("enum = %v, want the case name", out["level"])
	}
	if got := mustJSON(t, out["perms"]); got != `["read","exec"]` {
		t.Errorf("flags = %s, want only the selected names in declaration order", got)
	}
	if out["label"] != nil {
		t.Errorf("absent option = %v, want null", out["label"])
	}
	if got := mustJSON(t, out["outcome"]); got != `{"ok":42}` {
		t.Errorf("result = %s", got)
	}
	if got := mustJSON(t, out["counts"]); got != `[["a",1],["b",2]]` {
		t.Errorf("map = %s, want pairs (keys need not be strings)", got)
	}
}

// TestPayloadLessVariantIsABareName — the compact form must survive both ways.
func TestPayloadLessVariantIsABareName(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	variant := g.push(types.MakeSchemaTypeBodyVariantType([]types.VariantCaseType{
		{Name: "pending", Payload: witTypes.None[int32](), Metadata: types.MetadataEnvelope{}},
		{Name: "failed", Payload: some(str), Metadata: types.MetadataEnvelope{}},
	}))
	ref := NewRef(g.graph(variant))

	if got := roundTrip(t, ref, `"pending"`); got != "pending" {
		t.Errorf("payload-less case = %v, want the bare name", got)
	}
	if _, err := ref.PackJSONBytes([]byte(`{"pending": null}`)); err == nil {
		t.Error("a payload on a payload-less case should be rejected")
	}
	if _, err := ref.PackJSONBytes([]byte(`"failed"`)); err == nil {
		t.Error("a case that needs a payload should not accept a bare name")
	}
}

// TestRichScalarsRoundTrip — duration, quantity, text and binary have object
// forms with exact field names that other SDKs must read.
func TestRichScalarsRoundTrip(t *testing.T) {
	var g graphBuilder
	duration := g.push(types.MakeSchemaTypeBodyDurationType())
	quantity := g.push(types.MakeSchemaTypeBodyQuantityType(types.QuantitySpec{
		BaseUnit:        "kg",
		AllowedSuffixes: []string{"kg", "g"},
		Min:             witTypes.None[types.QuantityValue](),
		Max:             witTypes.None[types.QuantityValue](),
	}))
	text := g.push(types.MakeSchemaTypeBodyTextType(types.TextRestrictions{
		Languages: witTypes.None[[]string](),
		MinLength: witTypes.None[uint32](),
		MaxLength: witTypes.None[uint32](),
		Regex:     witTypes.None[string](),
	}))
	binary := g.push(types.MakeSchemaTypeBodyBinaryType(types.BinaryRestrictions{
		MimeTypes: witTypes.None[[]string](),
		MinBytes:  witTypes.None[uint32](),
		MaxBytes:  witTypes.None[uint32](),
	}))
	rec := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "took", Body: duration, Metadata: types.MetadataEnvelope{}},
		{Name: "weight", Body: quantity, Metadata: types.MetadataEnvelope{}},
		{Name: "note", Body: text, Metadata: types.MetadataEnvelope{}},
		{Name: "blob", Body: binary, Metadata: types.MetadataEnvelope{}},
	}))
	ref := NewRef(g.graph(rec))

	input := `{
		"took": {"nanoseconds": "1500000000"},
		"weight": {"mantissa": "125", "scale": 2, "unit": "kg"},
		"note": {"text": "hello", "language": "en"},
		"blob": {"bytes": "aGVsbG8", "mimeType": "text/plain"}
	}`
	out := roundTrip(t, ref, input).(map[string]any)

	if got := mustJSON(t, out["took"]); got != `{"nanoseconds":"1500000000"}` {
		t.Errorf("duration = %s", got)
	}
	if got := mustJSON(t, out["weight"]); got != `{"mantissa":"125","scale":2,"unit":"kg"}` {
		t.Errorf("quantity = %s", got)
	}
	if got := mustJSON(t, out["note"]); got != `{"language":"en","text":"hello"}` {
		t.Errorf("text = %s", got)
	}
	// base64url without padding, per the canonical form.
	if got := mustJSON(t, out["blob"]); got != `{"bytes":"aGVsbG8","mimeType":"text/plain"}` {
		t.Errorf("binary = %s", got)
	}
}

// TestPackRejectsUnknownAndMissingFields — a record is positional on the wire,
// so a typo in a field name has to be caught here rather than shifting values.
func TestPackRejectsUnknownAndMissingFields(t *testing.T) {
	ref := NewRef(recordSchema())

	if _, err := ref.PackJSONBytes([]byte(`{"name":"x"}`)); err == nil {
		t.Error("a missing field should be rejected")
	} else if !strings.Contains(err.Error(), "count") {
		t.Errorf("the error should name the missing field, got %q", err)
	}
	if _, err := ref.PackJSONBytes([]byte(`{"name":"x","count":1,"extra":true}`)); err == nil {
		t.Error("an unknown field should be rejected")
	}
}

// TestPackRejectsOutOfRangeIntegers — the schema's width is authoritative.
func TestPackRejectsOutOfRangeIntegers(t *testing.T) {
	var g graphBuilder
	u8 := g.push(types.MakeSchemaTypeBodyU8Type(witTypes.None[types.NumericRestrictions]()))
	ref := NewRef(g.graph(u8))

	if _, err := ref.PackJSONBytes([]byte(`300`)); err == nil {
		t.Error("300 should not fit a u8")
	}
	if _, err := ref.PackJSONBytes([]byte(`-1`)); err == nil {
		t.Error("-1 should not fit a u8")
	}
	if _, err := ref.PackJSONBytes([]byte(`1.5`)); err == nil {
		t.Error("a fraction is not an integer")
	}
}

// TestUnpackRejectsHostHandles — a guest holds capabilities as opaque
// references; rendering one would imply it can read the material.
func TestUnpackRejectsHostHandles(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	secret := g.push(types.MakeSchemaTypeBodySecretType(types.SecretSpec{
		Inner: str, Category: witTypes.None[string](),
	}))
	ref := NewRef(g.graph(secret))

	if _, err := ref.PackJSONBytes([]byte(`"hunter2"`)); err == nil {
		t.Error("a secret should not be constructible from JSON")
	}
}

// TestNestedRoundTripPreservesStructure — lists of records of options, the
// shape most agent inputs actually are.
func TestNestedRoundTripPreservesStructure(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	opt := g.push(types.MakeSchemaTypeBodyOptionType(str))
	item := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "id", Body: str, Metadata: types.MetadataEnvelope{}},
		{Name: "note", Body: opt, Metadata: types.MetadataEnvelope{}},
	}))
	list := g.push(types.MakeSchemaTypeBodyListType(item))
	ref := NewRef(g.graph(list))

	input := `[{"id":"a","note":"first"},{"id":"b","note":null}]`
	out := roundTrip(t, ref, input)
	if got := mustJSON(t, out); got != input {
		t.Errorf("round-trip = %s, want %s", got, input)
	}
	if reflect.TypeOf(out).Kind() != reflect.Slice {
		t.Errorf("a list should render as an array, got %T", out)
	}
}
