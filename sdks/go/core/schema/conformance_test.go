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
	"bytes"
	"encoding/json"
	"errors"
	"math"
	"os"
	"reflect"
	"regexp"
	"slices"
	"sort"
	"strings"
	"testing"
)

// The cross-SDK reflection conformance corpus: the same cases every SDK runs
// against its canonical JSON, validation and JSON Schema, so they cannot drift
// apart. Each case names a fixture that each SDK builds from its own model.
const conformanceCorpus = "../../../../test-data/reflection-conformance/v1.json"

// kindsBeyondCorpus are schema kinds the shared corpus does not enumerate yet.
var kindsBeyondCorpus = []string{"uuid"}

type conformanceCase struct {
	ID        string            `json:"id"`
	Operation string            `json:"operation"`
	Fixture   string            `json:"fixture"`
	Input     json.RawMessage   `json:"input"`
	Inputs    []json.RawMessage `json:"inputs"`
	Path      string            `json:"path"`
	Expected  json.RawMessage   `json:"expected"`
}

type conformanceFile struct {
	Version          string            `json:"version"`
	SchemaKinds      []string          `json:"schemaKinds"`
	RestrictionKinds []string          `json:"restrictionKinds"`
	CaseIDs          []string          `json:"caseIds"`
	Cases            []conformanceCase `json:"cases"`
}

func loadConformance(t *testing.T) conformanceFile {
	t.Helper()
	raw, err := os.ReadFile(conformanceCorpus)
	if err != nil {
		t.Fatalf("the reflection conformance corpus: %v", err)
	}
	var corpus conformanceFile
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatal(err)
	}
	return corpus
}

func croot(body SchemaTypeBody) Ref { return NewRef(SchemaGraph{Root: SchemaType{Body: body}}) }

func ct(body SchemaTypeBody) SchemaType { return SchemaType{Body: body} }

func bound(b NumericBound) *NumericBound { return &b }

func conformanceFixture(t *testing.T, name string) Ref {
	t.Helper()
	optionalString := ct(OptionType{Inner: ct(StringType{})})
	switch name {
	case "s64":
		return croot(S64Type{})
	case "constrained-s64":
		return croot(S64Type{Restrictions: &NumericRestrictions{
			Min: bound(signedBound(-9_007_199_254_740_993)), Max: bound(signedBound(9_007_199_254_740_993)),
		}})
	case "u64":
		return croot(U64Type{})
	case "binary":
		return croot(BinaryType{})
	case "duration":
		return croot(DurationType{})
	case "quantity":
		return croot(QuantityType{Spec: QuantitySpec{BaseUnit: "m", AllowedSuffixes: []string{}}})
	case "optional-record":
		return NewRef(SchemaGraph{
			Defs: []SchemaTypeDef{{Id: "conformance.optional", Body: optionalString}},
			Root: ct(RecordType{Fields: []NamedField{
				{Name: "direct", Body: optionalString},
				{Name: "referenced", Body: ct(RefType{Id: "conformance.optional"})},
			}}),
		})
	case "tool-input":
		return croot(RecordType{Fields: []NamedField{
			{Name: "pattern", Body: ct(StringType{})},
			{Name: "paths", Body: ct(ListType{Element: ct(StringType{})})},
			{Name: "ignoreCase", Body: ct(OptionType{Inner: ct(BoolType{})})},
		}})
	case "config-entry":
		return croot(RecordType{Fields: []NamedField{
			{Name: "path", Body: ct(ListType{Element: ct(StringType{})})},
			{Name: "value", Body: ct(S64Type{})},
		}})
	case "constrained-u32":
		return croot(U32Type{Restrictions: &NumericRestrictions{Min: bound(unsignedBound(2)), Max: bound(unsignedBound(10))}})
	case "constrained-f64":
		return croot(F64Type{Restrictions: &NumericRestrictions{Min: bound(floatBound(-1.5)), Max: bound(floatBound(2.5))}})
	case "constrained-text":
		languages, minLength, maxLength, regex := []string{"en", "de"}, uint32(2), uint32(8), "^[a-z]+$"
		return croot(TextType{Restrictions: TextRestrictions{
			Languages: &languages, MinLength: &minLength, MaxLength: &maxLength, Regex: &regex,
		}})
	case "constrained-binary":
		mimeTypes, minBytes, maxBytes := []string{"image/png", "application/octet-stream"}, uint32(2), uint32(4)
		return croot(BinaryType{Restrictions: BinaryRestrictions{
			MimeTypes: &mimeTypes, MinBytes: &minBytes, MaxBytes: &maxBytes,
		}})
	case "result":
		ok, err := ct(StringType{}), ct(U32Type{})
		return croot(ResultType{Ok: &ok, Err: &err})
	case "custom-error":
		ok := ct(StringType{})
		err := ct(RecordType{Fields: []NamedField{
			{Name: "code", Body: ct(StringType{})},
			{Name: "retryable", Body: ct(BoolType{})},
		}})
		return croot(ResultType{Ok: &ok, Err: &err})
	}
	t.Fatalf("unknown conformance fixture %q", name)
	return Ref{}
}

// canonical decodes JSON keeping every number exact.
func canonical(t *testing.T, raw []byte) any {
	t.Helper()
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		t.Fatalf("decoding %s: %v", raw, err)
	}
	return normalizeNumbers(v)
}

// normalizeNumbers makes 2 and 2.0 compare equal, as they are the same JSON
// number.
func normalizeNumbers(v any) any {
	switch x := v.(type) {
	case json.Number:
		if f, err := x.Float64(); err == nil && !strings.ContainsAny(x.String(), "eE") &&
			float64(int64(f)) == f && math.Abs(f) < 1<<53 {
			return int64(f)
		}
		f, _ := x.Float64()
		return f
	case map[string]any:
		for k, e := range x {
			x[k] = normalizeNumbers(e)
		}
	case []any:
		for i, e := range x {
			x[i] = normalizeNumbers(e)
		}
	}
	return v
}

// expectSubset requires every key of an expected object to be present and
// equal, recursively; anything else must be equal as a whole.
func expectSubset(t *testing.T, id string, actual, expected any) {
	t.Helper()
	want, ok := expected.(map[string]any)
	if !ok {
		if !reflect.DeepEqual(actual, expected) {
			t.Errorf("%s: got %#v, want %#v", id, actual, expected)
		}
		return
	}
	got, ok := actual.(map[string]any)
	if !ok {
		t.Errorf("%s: got %#v, want an object", id, actual)
		return
	}
	for k, v := range want {
		a, present := got[k]
		if !present {
			t.Errorf("%s: missing %q in %#v", id, k, got)
			continue
		}
		expectSubset(t, id+"."+k, a, v)
	}
}

func atPointer(t *testing.T, v any, pointer string) any {
	t.Helper()
	if pointer == "" {
		return v
	}
	for _, part := range strings.Split(pointer[1:], "/") {
		part = strings.ReplaceAll(strings.ReplaceAll(part, "~1", "/"), "~0", "~")
		m, ok := v.(map[string]any)
		if !ok {
			t.Fatalf("cannot resolve %s", pointer)
		}
		if v, ok = m[part]; !ok {
			t.Fatalf("missing %s", pointer)
		}
	}
	return v
}

func jsonSchemaOf(t *testing.T, r Ref) any {
	t.Helper()
	raw, err := r.ToJSONSchemaBytes(false)
	if err != nil {
		t.Fatalf("ToJSONSchema: %v", err)
	}
	return canonical(t, raw)
}

// restrictionKinds are the restrictions the Go model carries, one per field of
// the restriction and spec types.
var restrictionKinds = []string{
	"numeric-minimum", "numeric-maximum", "numeric-unit",
	"text-languages", "text-min-length", "text-max-length", "text-regex",
	"binary-mime-types", "binary-min-bytes", "binary-max-bytes",
	"path-direction", "path-kind", "path-mime-types", "path-extensions",
	"url-schemes", "url-hosts",
	"quantity-base-unit", "quantity-suffixes", "quantity-minimum", "quantity-maximum",
	"union-prefix", "union-suffix", "union-regex", "union-field",
}

var kindName = regexp.MustCompile(`^\{"kind":"([a-z0-9-]+)"`)

func runConformanceCase(t *testing.T, corpus conformanceFile, c conformanceCase) {
	switch c.Operation {
	case "roundtrip":
		r := conformanceFixture(t, c.Fixture)
		v, err := r.PackJSONBytes(c.Input)
		if err != nil {
			t.Fatalf("packing %s: %v", c.Input, err)
		}
		out, err := r.UnpackJSONBytes(v)
		if err != nil {
			t.Fatalf("unpacking: %v", err)
		}
		if got, want := canonical(t, out), canonical(t, c.Expected); !reflect.DeepEqual(got, want) {
			t.Fatalf("round-tripped to %s, want %s", out, c.Expected)
		}
	case "reject":
		r := conformanceFixture(t, c.Fixture)
		var want struct{ Kind string }
		_ = json.Unmarshal(c.Expected, &want)
		inputs := c.Inputs
		if len(inputs) == 0 {
			inputs = []json.RawMessage{c.Input}
		}
		for _, in := range inputs {
			_, err := r.PackJSONBytes(in)
			if err == nil {
				t.Errorf("packing %s succeeded, want %s", in, want.Kind)
				continue
			}
			var violation *ConstraintViolationError
			got := "invalid-json"
			if errors.As(err, &violation) {
				got = "constraint-violation"
			}
			if got != want.Kind {
				t.Errorf("packing %s failed as %s (%v), want %s", in, got, err, want.Kind)
			}
		}
	case "json-schema":
		schema := jsonSchemaOf(t, conformanceFixture(t, c.Fixture))
		expectSubset(t, c.ID, atPointer(t, schema, c.Path), canonical(t, c.Expected))
	case "semantic":
		runSemanticCase(t, corpus, c)
	default:
		t.Fatalf("unknown operation %q", c.Operation)
	}
}

func runSemanticCase(t *testing.T, corpus conformanceFile, c conformanceCase) {
	var expected map[string]json.RawMessage
	if err := json.Unmarshal(c.Expected, &expected); err != nil {
		t.Fatal(err)
	}
	switch c.Fixture {
	case "unsupported-leaves":
		str := ct(StringType{})
		leaves := []SchemaTypeBody{
			SecretType{Inner: str},
			QuotaTokenType{},
			PermissionCardType{},
			FutureType{Item: &str},
			StreamType{Item: &str},
		}
		var count int
		_ = json.Unmarshal(expected["count"], &count)
		if len(leaves) != count {
			t.Fatalf("%d leaves, the corpus expects %d", len(leaves), count)
		}
		for _, leaf := range leaves {
			expectSubset(t, reflect.TypeOf(leaf).Name(), jsonSchemaOf(t, croot(leaf)), canonical(t, expected["schema"]))
		}
	case "all-kinds":
		var names []string
		_ = json.Unmarshal(expected["names"], &names)
		var ours []string
		for _, k := range everyTypeKind() {
			m := kindName.FindStringSubmatch(k.wire)
			if m == nil {
				t.Fatalf("no kind in %s", k.wire)
			}
			if !slices.Contains(kindsBeyondCorpus, m[1]) {
				ours = append(ours, m[1])
			}
		}
		if !sameSet(ours, names) || !sameSet(corpus.SchemaKinds, names) {
			t.Fatalf("Go knows the schema kinds %v, the corpus %v", ours, names)
		}
	case "all-restrictions":
		var names []string
		_ = json.Unmarshal(expected["names"], &names)
		if !reflect.DeepEqual(restrictionKinds, names) || !reflect.DeepEqual(corpus.RestrictionKinds, names) {
			t.Fatalf("Go carries the restrictions %v, the corpus %v", restrictionKinds, names)
		}
	case "graph":
		referenced := conformanceFixture(t, "optional-record")
		optionalString := ct(OptionType{Inner: ct(StringType{})})
		inline := croot(RecordType{Fields: []NamedField{
			{Name: "direct", Body: optionalString},
			{Name: "referenced", Body: optionalString},
		}})
		a, errA := referenced.PackJSONBytes([]byte(`{}`))
		b, errB := inline.PackJSONBytes([]byte(`{}`))
		if errA != nil || errB != nil {
			t.Fatalf("packing {}: %v / %v", errA, errB)
		}
		if !reflect.DeepEqual(a, b) {
			t.Fatalf("a referenced option packed to %#v, inline to %#v", a, b)
		}
	default:
		t.Fatalf("unknown semantic fixture %q", c.Fixture)
	}
}

func sameSet(a, b []string) bool {
	x, y := append([]string(nil), a...), append([]string(nil), b...)
	sort.Strings(x)
	sort.Strings(y)
	return reflect.DeepEqual(x, y)
}

func TestReflectionConformanceCorpus(t *testing.T) {
	corpus := loadConformance(t)
	if corpus.Version != "1.0.0" {
		t.Fatalf("corpus version %s; this runner reads 1.0.0", corpus.Version)
	}
	ids := make([]string, 0, len(corpus.Cases))
	for _, c := range corpus.Cases {
		ids = append(ids, c.ID)
	}
	if !sameSet(ids, corpus.CaseIDs) || len(ids) != len(corpus.CaseIDs) {
		t.Fatal("the corpus cases do not match its declared case IDs")
	}
	for _, c := range corpus.Cases {
		t.Run(c.ID, func(t *testing.T) { runConformanceCase(t, corpus, c) })
	}
}
