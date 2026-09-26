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
	"fmt"
	"time"
)

// The schema-native wire form.
//
// This is how the model itself travels over REST: every node is
// {"kind": "<case>", "value": <payload>}, with kebab-cased case names and
// camelCased payload fields. It is the serde shape of the server's Rust
// SchemaType and SchemaValue, and openapi/golem-service.yaml is generated from
// those same types.
//
// It is not canonical JSON, and confusing the two is the GOL-653 failure mode.
// Canonical JSON (json.go) renders a value *through its schema* into what an
// author would write by hand: a record becomes an object with named fields, a
// s64 becomes a base-10 string, binary becomes base64url. The wire form is
// structural and schema-free: a record is a positional list of nodes, each
// still carrying its own kind, a s64 is a JSON number, and binary is an array
// of byte numbers. Marshal*/Unmarshal* here, Pack*/Unpack* there.
//
// Values travel both ways; a type graph only ever arrives, since a caller
// reads a schema it was given rather than inventing one.

// MarshalWireValue renders a value in the schema-native wire form.
func MarshalWireValue(v SchemaValue) ([]byte, error) {
	node, err := valueToWire(v)
	if err != nil {
		return nil, err
	}
	return json.Marshal(node)
}

// UnmarshalWireValue reads a value in the schema-native wire form. The result
// carries no schema, so it is only meaningful beside the type it was built
// against.
func UnmarshalWireValue(data []byte) (SchemaValue, error) {
	var node wireNode
	if err := json.Unmarshal(data, &node); err != nil {
		return nil, fmt.Errorf("golem: malformed schema value: %w", err)
	}
	return wireToValue(node)
}

// UnmarshalWireGraph reads a type graph in the schema-native wire form.
func UnmarshalWireGraph(data []byte) (SchemaGraph, error) {
	var g wireGraph
	if err := json.Unmarshal(data, &g); err != nil {
		return SchemaGraph{}, fmt.Errorf("golem: malformed schema graph: %w", err)
	}
	return g.toModel()
}

// wireNode is the tagged envelope every type and value node travels in.
type wireNode struct {
	Kind  string          `json:"kind"`
	Value json.RawMessage `json:"value,omitempty"`
}

// wireValueKinds and wireTypeKinds are how many cases of each sum the wire form
// knows. A case added to the model without a case here would fall through to an
// "unsupported" error at runtime rather than failing to compile — Go switches
// are not exhaustive — so the counts are pinned by tests that enumerate every
// case.
const (
	wireValueKinds = 35
	wireTypeKinds  = 37
)

// --- Values, outgoing ----------------------------------------------------

func valueToWire(v SchemaValue) (wireNode, error) {
	switch n := v.(type) {
	case BoolValue:
		return wireScalar("bool", n.Value)
	case S8Value:
		return wireScalar("s8", n.Value)
	case S16Value:
		return wireScalar("s16", n.Value)
	case S32Value:
		return wireScalar("s32", n.Value)
	case S64Value:
		return wireScalar("s64", n.Value)
	case U8Value:
		return wireScalar("u8", n.Value)
	case U16Value:
		return wireScalar("u16", n.Value)
	case U32Value:
		return wireScalar("u32", n.Value)
	case U64Value:
		return wireScalar("u64", n.Value)
	case F32Value:
		return wireScalar("f32", n.Value)
	case F64Value:
		return wireScalar("f64", n.Value)
	case CharValue:
		if !validCodePoint(n.Value) {
			return wireNode{}, fmt.Errorf("golem: char %d is not a Unicode scalar value", n.Value)
		}
		return wireScalar("char", string(n.Value))
	case StringValue:
		return wireScalar("string", n.Value)

	case RecordValue:
		fields, err := valuesToWire(n.Fields)
		if err != nil {
			return wireNode{}, err
		}
		return wireScalar("record", map[string]any{"fields": fields})
	case VariantValue:
		payload := map[string]any{"case": n.Case}
		if n.Payload != nil {
			inner, err := valueToWire(*n.Payload)
			if err != nil {
				return wireNode{}, err
			}
			payload["payload"] = inner
		}
		return wireScalar("variant", payload)
	case EnumValue:
		return wireScalar("enum", map[string]any{"case": n.Case})
	case FlagsValue:
		return wireScalar("flags", map[string]any{"bits": nonNilBools(n.Set)})
	case TupleValue:
		elements, err := valuesToWire(n.Elements)
		if err != nil {
			return wireNode{}, err
		}
		return wireScalar("tuple", map[string]any{"elements": elements})
	case ListValue:
		items, err := valuesToWire(n.Items)
		if err != nil {
			return wireNode{}, err
		}
		return wireScalar("list", map[string]any{"elements": items})
	case FixedListValue:
		items, err := valuesToWire(n.Items)
		if err != nil {
			return wireNode{}, err
		}
		return wireScalar("fixed-list", map[string]any{"elements": items})
	case MapValue:
		entries := make([][2]wireNode, 0, len(n.Entries))
		for _, entry := range n.Entries {
			key, err := valueToWire(entry.Key)
			if err != nil {
				return wireNode{}, err
			}
			value, err := valueToWire(entry.Value)
			if err != nil {
				return wireNode{}, err
			}
			entries = append(entries, [2]wireNode{key, value})
		}
		return wireScalar("map", map[string]any{"entries": entries})
	case OptionValue:
		payload := map[string]any{}
		if n.Value != nil {
			inner, err := valueToWire(*n.Value)
			if err != nil {
				return wireNode{}, err
			}
			payload["inner"] = inner
		}
		return wireScalar("option", payload)
	case ResultValue:
		tag := "ok"
		if n.IsErr {
			tag = "err"
		}
		payload := map[string]any{"tag": tag}
		if n.Value != nil {
			inner, err := valueToWire(*n.Value)
			if err != nil {
				return wireNode{}, err
			}
			payload["value"] = inner
		}
		return wireScalar("result", payload)

	case TextValue:
		payload := map[string]any{"text": n.Text}
		if n.Language != nil {
			payload["language"] = *n.Language
		}
		return wireScalar("text", payload)
	case BinaryValue:
		payload := map[string]any{"bytes": byteNumbers(n.Bytes)}
		if n.MimeType != nil {
			payload["mimeType"] = *n.MimeType
		}
		return wireScalar("binary", payload)
	case PathValue:
		return wireScalar("path", map[string]any{"path": n.Value})
	case UrlValue:
		return wireScalar("url", map[string]any{"url": n.Value})
	case DatetimeValue:
		instant := time.Unix(n.Seconds, int64(n.Nanoseconds)).UTC()
		return wireScalar("datetime", map[string]any{"value": instant.Format(time.RFC3339Nano)})
	case DurationValue:
		return wireScalar("duration", map[string]any{"nanoseconds": n.Nanoseconds})
	case QuantityValueNode:
		return wireScalar("quantity", map[string]any{
			"mantissa": n.Value.Mantissa,
			"scale":    n.Value.Scale,
			"unit":     n.Value.Unit,
		})
	case UnionValue:
		body, err := valueToWire(n.Body)
		if err != nil {
			return wireNode{}, err
		}
		return wireScalar("union", map[string]any{"tag": n.Tag, "body": body})

	case SecretValue, QuotaTokenValue, PermissionCardValue, StreamValue:
		return wireNode{}, fmt.Errorf(
			"golem: %T is a handle to something the host holds, and does not travel over REST", v)
	}
	return wireNode{}, fmt.Errorf("golem: unsupported schema value %T", v)
}

func wireScalar(kind string, payload any) (wireNode, error) {
	raw, err := json.Marshal(payload)
	if err != nil {
		return wireNode{}, fmt.Errorf("golem: %s value: %w", kind, err)
	}
	return wireNode{Kind: kind, Value: raw}, nil
}

func valuesToWire(values []SchemaValue) ([]wireNode, error) {
	out := make([]wireNode, 0, len(values))
	for _, v := range values {
		node, err := valueToWire(v)
		if err != nil {
			return nil, err
		}
		out = append(out, node)
	}
	return out, nil
}

// --- Values, incoming ----------------------------------------------------

func wireToValue(node wireNode) (SchemaValue, error) {
	switch node.Kind {
	case "bool":
		return readWire(node, func(v bool) SchemaValue { return BoolValue{Value: v} })
	case "s8":
		return readWire(node, func(v int8) SchemaValue { return S8Value{Value: v} })
	case "s16":
		return readWire(node, func(v int16) SchemaValue { return S16Value{Value: v} })
	case "s32":
		return readWire(node, func(v int32) SchemaValue { return S32Value{Value: v} })
	case "s64":
		return readWire(node, func(v int64) SchemaValue { return S64Value{Value: v} })
	case "u8":
		return readWire(node, func(v uint8) SchemaValue { return U8Value{Value: v} })
	case "u16":
		return readWire(node, func(v uint16) SchemaValue { return U16Value{Value: v} })
	case "u32":
		return readWire(node, func(v uint32) SchemaValue { return U32Value{Value: v} })
	case "u64":
		return readWire(node, func(v uint64) SchemaValue { return U64Value{Value: v} })
	case "f32":
		return readWire(node, func(v float32) SchemaValue { return F32Value{Value: v} })
	case "f64":
		return readWire(node, func(v float64) SchemaValue { return F64Value{Value: v} })
	case "string":
		return readWire(node, func(v string) SchemaValue { return StringValue{Value: v} })

	case "char":
		return readWireErr(node, func(s string) (SchemaValue, error) {
			runes := []rune(s)
			if len(runes) != 1 || !validCodePoint(runes[0]) {
				return nil, fmt.Errorf("golem: char must be one Unicode scalar value, got %q", s)
			}
			return CharValue{Value: runes[0]}, nil
		})

	case "record":
		return readWireErr(node, func(p wireValueFields) (SchemaValue, error) {
			fields, err := wireToValues(p.Fields)
			return RecordValue{Fields: fields}, err
		})
	case "variant":
		return readWireErr(node, func(p wireVariantValue) (SchemaValue, error) {
			payload, err := wireToOptionalValue(p.Payload)
			return VariantValue{Case: p.Case, Payload: payload}, err
		})
	case "enum":
		return readWire(node, func(p wireCase) SchemaValue { return EnumValue(p) })
	case "flags":
		return readWire(node, func(p wireFlagsValue) SchemaValue { return FlagsValue{Set: p.Bits} })
	case "tuple":
		return readWireErr(node, func(p wireValueElements) (SchemaValue, error) {
			elements, err := wireToValues(p.Elements)
			return TupleValue{Elements: elements}, err
		})
	case "list":
		return readWireErr(node, func(p wireValueElements) (SchemaValue, error) {
			items, err := wireToValues(p.Elements)
			return ListValue{Items: items}, err
		})
	case "fixed-list":
		return readWireErr(node, func(p wireValueElements) (SchemaValue, error) {
			items, err := wireToValues(p.Elements)
			return FixedListValue{Items: items}, err
		})
	case "map":
		return readWireErr(node, func(p wireMapValue) (SchemaValue, error) {
			entries := make([]MapEntry, 0, len(p.Entries))
			for _, pair := range p.Entries {
				if len(pair) != 2 {
					return nil, fmt.Errorf(
						"golem: map entry must be a [key, value] pair, got %d element(s)", len(pair))
				}
				key, err := wireToValue(pair[0])
				if err != nil {
					return nil, err
				}
				value, err := wireToValue(pair[1])
				if err != nil {
					return nil, err
				}
				entries = append(entries, MapEntry{Key: key, Value: value})
			}
			return MapValue{Entries: entries}, nil
		})
	case "option":
		return readWireErr(node, func(p wireOptionValue) (SchemaValue, error) {
			inner, err := wireToOptionalValue(p.Inner)
			return OptionValue{Value: inner}, err
		})
	case "result":
		return readWireErr(node, func(p wireResultValue) (SchemaValue, error) {
			isErr := false
			switch p.Tag {
			case "ok":
			case "err":
				isErr = true
			default:
				return nil, fmt.Errorf("golem: result tag must be ok or err, got %q", p.Tag)
			}
			value, err := wireToOptionalValue(p.Value)
			return ResultValue{IsErr: isErr, Value: value}, err
		})

	case "text":
		return readWire(node, func(p wireTextValue) SchemaValue { return TextValue(p) })
	case "binary":
		return readWire(node, func(p wireBinaryValue) SchemaValue { return BinaryValue(p) })
	case "path":
		return readWire(node, func(p wirePathValue) SchemaValue { return PathValue{Value: p.Path} })
	case "url":
		return readWire(node, func(p wireUrlValue) SchemaValue { return UrlValue{Value: p.Url} })
	case "datetime":
		return readWireErr(node, func(p wireDatetimeValue) (SchemaValue, error) {
			instant, err := time.Parse(time.RFC3339Nano, p.Value)
			if err != nil {
				return nil, fmt.Errorf("golem: datetime %q: %w", p.Value, err)
			}
			return DatetimeValue{
				Seconds:     instant.Unix(),
				Nanoseconds: uint32(instant.Nanosecond()),
			}, nil
		})
	case "duration":
		return readWire(node, func(p wireDurationValue) SchemaValue { return DurationValue(p) })
	case "quantity":
		return readWire(node, func(p wireQuantityValue) SchemaValue {
			return QuantityValueNode{Value: QuantityValue(p)}
		})
	case "union":
		return readWireErr(node, func(p wireUnionValue) (SchemaValue, error) {
			body, err := wireToValue(p.Body)
			if err != nil {
				return nil, err
			}
			return UnionValue{Tag: p.Tag, Body: body}, nil
		})

	case "secret", "quota-token", "permission-card", "stream":
		return nil, fmt.Errorf(
			"golem: a %s is a handle to something the host holds, and cannot be read here", node.Kind)
	}
	return nil, fmt.Errorf("golem: unsupported schema value kind %q", node.Kind)
}

// readWire decodes a node's payload and wraps it; readWireErr does the same for
// a case that can still reject what it decoded.
func readWire[T any](node wireNode, wrap func(T) SchemaValue) (SchemaValue, error) {
	return readWireErr(node, func(v T) (SchemaValue, error) { return wrap(v), nil })
}

func readWireErr[T any](node wireNode, wrap func(T) (SchemaValue, error)) (SchemaValue, error) {
	var payload T
	if err := json.Unmarshal(node.Value, &payload); err != nil {
		return nil, fmt.Errorf("golem: %s value: %w", node.Kind, err)
	}
	return wrap(payload)
}

func wireToValues(nodes []wireNode) ([]SchemaValue, error) {
	out := make([]SchemaValue, 0, len(nodes))
	for _, node := range nodes {
		v, err := wireToValue(node)
		if err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	return out, nil
}

// wireToOptionalValue treats an omitted field and an explicit null the same, so
// the server's serde output (which renders an empty payload as null) reads back
// as the omitted form other SDKs send.
func wireToOptionalValue(node *wireNode) (*SchemaValue, error) {
	if node == nil || node.Kind == "" {
		return nil, nil
	}
	v, err := wireToValue(*node)
	if err != nil {
		return nil, err
	}
	return &v, nil
}

type wireValueFields struct {
	Fields []wireNode `json:"fields"`
}

type wireValueElements struct {
	Elements []wireNode `json:"elements"`
}

type wireVariantValue struct {
	Case    uint32    `json:"case"`
	Payload *wireNode `json:"payload"`
}

type wireCase struct {
	Case uint32 `json:"case"`
}

type wireFlagsValue struct {
	Bits []bool `json:"bits"`
}

type wireMapValue struct {
	Entries [][]wireNode `json:"entries"`
}

type wireOptionValue struct {
	Inner *wireNode `json:"inner"`
}

type wireResultValue struct {
	Tag   string    `json:"tag"`
	Value *wireNode `json:"value"`
}

type wireTextValue struct {
	Text     string  `json:"text"`
	Language *string `json:"language"`
}

type wireBinaryValue struct {
	Bytes    []byte  `json:"-"`
	MimeType *string `json:"mimeType"`
}

// UnmarshalJSON reads the byte array the server's serde derive emits. Go's
// encoding/json would otherwise expect a base64 string for a []byte field.
func (b *wireBinaryValue) UnmarshalJSON(data []byte) error {
	var raw struct {
		Bytes    []uint8 `json:"bytes"`
		MimeType *string `json:"mimeType"`
	}
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}
	b.Bytes = raw.Bytes
	b.MimeType = raw.MimeType
	return nil
}

type wirePathValue struct {
	Path string `json:"path"`
}

type wireUrlValue struct {
	Url string `json:"url"`
}

type wireDatetimeValue struct {
	Value string `json:"value"`
}

type wireDurationValue struct {
	Nanoseconds int64 `json:"nanoseconds"`
}

type wireQuantityValue struct {
	Mantissa int64  `json:"mantissa"`
	Scale    int32  `json:"scale"`
	Unit     string `json:"unit"`
}

type wireUnionValue struct {
	Tag  string   `json:"tag"`
	Body wireNode `json:"body"`
}

// byteNumbers renders bytes the way the server's serde derive does: an array
// of numbers. Go would encode a []byte as base64, which is canonical JSON's
// form, not the wire form's.
func byteNumbers(b []byte) []int {
	out := make([]int, len(b))
	for i, v := range b {
		out[i] = int(v)
	}
	return out
}

func nonNilBools(v []bool) []bool {
	if v == nil {
		return []bool{}
	}
	return v
}

func validCodePoint(r rune) bool {
	return r >= 0 && r <= 0x10ffff && (r < 0xd800 || r > 0xdfff)
}
