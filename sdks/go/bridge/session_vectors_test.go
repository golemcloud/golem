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
	"bytes"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// The protocol's normative vectors, shared by every runtime.
const vectorDir = "../../../golem-client/tests/fixtures/stream-session-v1"

func loadVectors(t *testing.T, name string) []map[string]any {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(vectorDir, name))
	if err != nil {
		t.Fatal(err)
	}
	var file struct {
		Vectors []map[string]any `json:"vectors"`
	}
	if err := json.Unmarshal(data, &file); err != nil {
		t.Fatal(err)
	}
	return file.Vectors
}

// fixtureType turns the vectors' compact schema notation into a type.
func fixtureType(t *testing.T, raw any) schema.SchemaType {
	t.Helper()
	m := raw.(map[string]any)
	ty := func(body schema.SchemaTypeBody) schema.SchemaType { return schema.SchemaType{Body: body} }
	sub := func(name string) schema.SchemaType { return fixtureType(t, m[name]) }
	optional := func(name string) *schema.SchemaType {
		if m[name] == nil {
			return nil
		}
		out := sub(name)
		return &out
	}
	strings := func(name string) []string {
		var out []string
		for _, s := range m[name].([]any) {
			out = append(out, s.(string))
		}
		return out
	}
	switch m["kind"] {
	case "bool":
		return ty(schema.BoolType{})
	case "s8":
		return ty(schema.S8Type{})
	case "s32":
		return ty(schema.S32Type{})
	case "s64":
		return ty(schema.S64Type{})
	case "u8":
		return ty(schema.U8Type{})
	case "u32":
		return ty(schema.U32Type{})
	case "u64":
		return ty(schema.U64Type{})
	case "f32":
		return ty(schema.F32Type{})
	case "f64":
		return ty(schema.F64Type{})
	case "char":
		return ty(schema.CharType{})
	case "string":
		return ty(schema.StringType{})
	case "text":
		return ty(schema.TextType{})
	case "binary":
		var rs schema.BinaryRestrictions
		if m["mimeTypes"] != nil {
			types := strings("mimeTypes")
			rs.MimeTypes = &types
		}
		if n, ok := m["maxBytes"].(float64); ok {
			max := uint32(n)
			rs.MaxBytes = &max
		}
		return ty(schema.BinaryType{Restrictions: rs})
	case "path":
		return ty(schema.PathType{})
	case "url":
		return ty(schema.UrlType{})
	case "datetime":
		return ty(schema.DatetimeType{})
	case "duration":
		return ty(schema.DurationType{})
	case "quantity":
		return ty(schema.QuantityType{Spec: schema.QuantitySpec{BaseUnit: "kg"}})
	case "ref":
		return ty(schema.RefType{Id: m["name"].(string)})
	case "record":
		var fields []schema.NamedField
		for _, f := range m["fields"].([]any) {
			field := f.(map[string]any)
			fields = append(fields, schema.NamedField{Name: field["name"].(string), Body: fixtureType(t, field["type"])})
		}
		return ty(schema.RecordType{Fields: fields})
	case "tuple":
		var elements []schema.SchemaType
		for _, e := range m["elements"].([]any) {
			elements = append(elements, fixtureType(t, e))
		}
		return ty(schema.TupleType{Elements: elements})
	case "list":
		return ty(schema.ListType{Element: sub("element")})
	case "fixed-list":
		return ty(schema.FixedListType{Element: sub("element"), Length: uint32(m["length"].(float64))})
	case "map":
		return ty(schema.MapType{Key: sub("key"), Value: sub("value")})
	case "enum":
		return ty(schema.EnumType{Cases: strings("cases")})
	case "flags":
		return ty(schema.FlagsType{Flags: strings("flags")})
	case "variant":
		var cases []schema.VariantCase
		for _, c := range m["cases"].([]any) {
			vc := c.(map[string]any)
			var payload *schema.SchemaType
			if vc["type"] != nil {
				p := fixtureType(t, vc["type"])
				payload = &p
			}
			cases = append(cases, schema.VariantCase{Name: vc["name"].(string), Payload: payload})
		}
		return ty(schema.VariantType{Cases: cases})
	case "option":
		return ty(schema.OptionType{Inner: sub("inner")})
	case "result":
		return ty(schema.ResultType{Ok: optional("ok"), Err: optional("err")})
	case "union":
		var branches []schema.UnionBranch
		for _, b := range m["branches"].([]any) {
			branch := b.(map[string]any)
			rule := branch["discriminator"].(map[string]any)
			if rule["rule"] != "prefix" {
				t.Fatalf("unknown discriminator %v", rule)
			}
			branches = append(branches, schema.UnionBranch{
				Tag: branch["name"].(string), Body: fixtureType(t, branch["type"]),
				Discriminator: schema.PrefixRule{Value: rule["prefix"].(string)},
			})
		}
		return ty(schema.UnionType{Branches: branches})
	case "stream":
		return ty(schema.StreamType{Item: optional("inner")})
	}
	t.Fatalf("unknown fixture schema kind %v", m["kind"])
	return schema.SchemaType{}
}

func fixtureRef(t *testing.T, vector map[string]any) schema.Ref {
	graph := schema.SchemaGraph{Root: fixtureType(t, vector["schema"])}
	if defs, ok := vector["definitions"].(map[string]any); ok {
		for id, body := range defs {
			graph.Defs = append(graph.Defs, schema.SchemaTypeDef{Id: id, Body: fixtureType(t, body)})
		}
	}
	return schema.NewRef(graph)
}

// readSessionValue is what the session does to a value it receives: read it
// strictly, validate it against its type, and check no stream appears twice.
func readSessionValue(ref schema.Ref, data []byte) (schema.SchemaValue, error) {
	v, err := schema.UnmarshalWireValue(data)
	if err != nil {
		return nil, err
	}
	if err := ref.Validate(v); err != nil {
		return nil, err
	}
	seen := map[schema.WireStreamRef]bool{}
	_, err = mapStreams(v, func(sv schema.StreamValue) (schema.SchemaValue, error) {
		wire := sv.Handle.(schema.WireStreamRef)
		if seen[wire] {
			return nil, errors.New("stream-already-consumed")
		}
		seen[wire] = true
		return sv, nil
	})
	return v, err
}

func TestSchemaValueVectorsRoundTrip(t *testing.T) {
	for _, vector := range loadVectors(t, "schema-values.json") {
		name := vector["name"].(string)
		canonical := vector["canonical"].(string)
		v, err := readSessionValue(fixtureRef(t, vector), []byte(canonical))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		out, err := schema.MarshalWireValue(v)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if string(out) != canonical {
			t.Fatalf("%s: re-encoded as\n%s\nwant\n%s", name, out, canonical)
		}
	}
}

func TestMalformedVectorsAreRejected(t *testing.T) {
	for _, vector := range loadVectors(t, "malformed.json") {
		name := vector["name"].(string)
		switch vector["lane"] {
		case "schema-value":
			if _, err := readSessionValue(fixtureRef(t, vector), []byte(vector["input"].(string))); err == nil {
				t.Errorf("%s: accepted", name)
			}
		case "binary":
			data, _ := hex.DecodeString(vector["inputHex"].(string))
			if _, err := decodeBinary(data); err == nil {
				t.Errorf("%s: accepted", name)
			}
		case "text":
			var data []byte
			if raw, ok := vector["inputBase64"].(string); ok {
				data, _ = base64.StdEncoding.DecodeString(raw)
			} else {
				data = []byte(vector["input"].(string))
			}
			if err := readClientMessage(data); err == nil && vector["expectedCode"] != "protocol-error" &&
				vector["expectedCode"] != "validation-error" {
				t.Errorf("%s: accepted", name)
			}
		case "token":
			if raw, ok := vector["input"].(string); ok && raw == "" {
				if checkToken(raw) == nil {
					t.Errorf("%s: accepted", name)
				}
			}
			if shape, ok := vector["inputShape"].(map[string]any); ok {
				token := shape["prefix"].(string) + string(bytes.Repeat([]byte(shape["ascii"].(string)), int(shape["length"].(float64))-len(shape["prefix"].(string))))
				if checkToken(token) == nil {
					t.Errorf("%s: accepted", name)
				}
			}
		}
	}
}

// readClientMessage applies the checks every message must pass, whichever
// direction it travels: the envelope, the known variants and their members,
// and canonical decimal strings for sequences.
func readClientMessage(data []byte) error {
	kind, m, err := parseMessage(data)
	if err != nil {
		return err
	}
	switch kind {
	case "inputStreamEnd":
		if err := m.members(kind, []string{"channel", "sequence"}); err != nil {
			return err
		}
		if _, err := m.channel(); err != nil {
			return err
		}
		_, err := m.u64("sequence")
		return err
	case "streamCancel":
		return m.members(kind, []string{"channel", "reason"})
	case "resumeAttach":
		return m.members(kind, []string{"attemptId", "operation", "outputCursors", "sessionToken"})
	}
	return protocolErrorf("unknown message type %q", kind)
}

func TestClientMessagesAreCanonical(t *testing.T) {
	for _, vector := range loadVectors(t, "json-messages.json") {
		if vector["direction"] != "client" {
			continue
		}
		canonical := vector["canonical"].(string)
		var members map[string]json.RawMessage
		if err := json.Unmarshal([]byte(canonical), &members); err != nil {
			t.Fatal(err)
		}
		kind := members["type"]
		delete(members, "type")
		delete(members, "version")
		var typ string
		_ = json.Unmarshal(kind, &typ)
		asAny := map[string]any{}
		for k, v := range members {
			asAny[k] = v
		}
		out, err := encodeMessage(typ, asAny)
		if err != nil {
			t.Fatal(err)
		}
		if string(out) != canonical {
			t.Fatalf("%s: encoded as\n%s\nwant\n%s", vector["name"], out, canonical)
		}
	}
}

func TestServerMessagesParse(t *testing.T) {
	for _, vector := range loadVectors(t, "json-messages.json") {
		if vector["direction"] != "server" {
			continue
		}
		kind, m, err := parseMessage([]byte(vector["canonical"].(string)))
		if err != nil || kind != vector["type"] {
			t.Fatalf("%s: %v", vector["name"], err)
		}
		if m.has("mappings") {
			if _, err := m.mappings(); err != nil {
				t.Fatalf("%s: %v", vector["name"], err)
			}
		}
	}
}

func TestBinaryVectors(t *testing.T) {
	for _, vector := range loadVectors(t, "binary-messages.json") {
		name := vector["name"].(string)
		frame, _ := base64.StdEncoding.DecodeString(vector["frameBase64"].(string))
		payload, _ := hex.DecodeString(vector["payloadHex"].(string))
		decoded, err := decodeBinary(frame)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if !bytes.Equal(decoded.payload, payload) {
			t.Fatalf("%s: payload %x", name, decoded.payload)
		}
		if decoded.kind == "input-u8" || decoded.kind == "input-binary" {
			kind := decoded.kind
			encoded, err := encodeBinary(kind, decoded.channel, decoded.sequence, int(decoded.count), decoded.mime, payload)
			if err != nil || !bytes.Equal(encoded, frame) {
				t.Fatalf("%s: encoded as %q, %v", name, encoded, err)
			}
		}
	}
}
