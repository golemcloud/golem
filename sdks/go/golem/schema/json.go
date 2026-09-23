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
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"regexp"
	"strconv"
	"time"

	witTypes "go.bytecodealliance.org/pkg/wit/types"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// The canonical JSON form is the cross-language contract: the same value must
// render identically from every SDK and the host, because it appears in the
// REST API, the CLI, oplog rendering and tool I/O.
//
// The rule that bites: integers up to 32 bits are JSON numbers, but s64, u64,
// duration nanoseconds and quantity mantissas are base-10 STRINGS. JSON numbers
// are doubles in most readers, which silently rounds anything past 2^53; a
// string cannot. Two other SDKs clamp those to the safe range instead — that
// loses values the host will happily send, so this follows the host.
//
//	bool                  true
//	s8/s16/s32, u8/u16/u32, f32/f64
//	                      42
//	s64, u64              "9007199254740993"
//	char, string          "a", "text"
//	text                  {"text": "...", "language": "en"}
//	binary                {"bytes": "<base64url, no padding>", "mimeType": "..."}
//	path, url             "..."
//	datetime              "2026-09-23T12:00:00Z"
//	duration              {"nanoseconds": "1500000000"}
//	quantity              {"mantissa": "125", "scale": 2, "unit": "kg"}
//	record                {"field": ...}
//	variant               "caseName" | {"caseName": payload}
//	enum                  "caseName"
//	flags                 ["selected", "names"]
//	tuple, list           [...]
//	map                   [[key, value], ...]
//	option                null | inner
//	result                {"ok": ...} | {"err": ...}
//	union                 the branch body's own JSON

var canonicalSigned = regexp.MustCompile(`^-?(0|[1-9][0-9]*)$`)

// checkedIntegerString parses a canonical base-10 integer string. Leading
// zeroes, a leading `+` and `-0` are rejected so that one value has one
// spelling — receivers compare these as strings.
func checkedIntegerString(raw string, label string) (int64, error) {
	if !canonicalSigned.MatchString(raw) || raw == "-0" {
		return 0, fmt.Errorf("%s must be a canonical base-10 integer string, found %q", label, raw)
	}
	n, err := strconv.ParseInt(raw, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("%s is out of range for a 64-bit signed integer: %q", label, raw)
	}
	return n, nil
}

func checkedUnsignedString(raw string, label string) (uint64, error) {
	if !canonicalSigned.MatchString(raw) || raw == "" || raw[0] == '-' {
		return 0, fmt.Errorf("%s must be a canonical base-10 unsigned integer string, found %q", label, raw)
	}
	n, err := strconv.ParseUint(raw, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("%s is out of range for a 64-bit unsigned integer: %q", label, raw)
	}
	return n, nil
}

// UnpackJSON renders a value as canonical JSON. The value is validated first,
// so a caller never renders something a receiver would reject.
func (r Ref) UnpackJSON(value types.SchemaValueTree) (any, error) {
	if err := r.Validate(value); err != nil {
		return nil, err
	}
	u := &unpacker{ref: r, value: value}
	return u.render(r.root, value.Root, "")
}

// UnpackJSONBytes is [Ref.UnpackJSON] serialised, for callers that want bytes.
func (r Ref) UnpackJSONBytes(value types.SchemaValueTree) ([]byte, error) {
	rendered, err := r.UnpackJSON(value)
	if err != nil {
		return nil, err
	}
	return json.Marshal(rendered)
}

type unpacker struct {
	ref   Ref
	value types.SchemaValueTree
}

func (u *unpacker) render(typeIdx, valueIdx int32, path string) (any, error) {
	body, _, err := u.ref.node(typeIdx)
	if err != nil {
		return nil, err
	}
	if valueIdx < 0 || int(valueIdx) >= len(u.value.ValueNodes) {
		return nil, fmt.Errorf("%s: value node index %d is out of range", pathOrRoot(path), valueIdx)
	}
	node := u.value.ValueNodes[valueIdx]

	switch body.Tag() {
	case types.SchemaTypeBodyBoolType:
		return node.BoolValue(), nil
	case types.SchemaTypeBodyS8Type:
		return int64(node.S8Value()), nil
	case types.SchemaTypeBodyS16Type:
		return int64(node.S16Value()), nil
	case types.SchemaTypeBodyS32Type:
		return int64(node.S32Value()), nil
	case types.SchemaTypeBodyS64Type:
		// Wide integers travel as strings; see the table above.
		return strconv.FormatInt(node.S64Value(), 10), nil
	case types.SchemaTypeBodyU8Type:
		return uint64(node.U8Value()), nil
	case types.SchemaTypeBodyU16Type:
		return uint64(node.U16Value()), nil
	case types.SchemaTypeBodyU32Type:
		return uint64(node.U32Value()), nil
	case types.SchemaTypeBodyU64Type:
		return strconv.FormatUint(node.U64Value(), 10), nil
	case types.SchemaTypeBodyF32Type:
		return float64(node.F32Value()), nil
	case types.SchemaTypeBodyF64Type:
		return node.F64Value(), nil
	case types.SchemaTypeBodyCharType:
		return string(node.CharValue()), nil
	case types.SchemaTypeBodyStringType:
		return node.StringValue(), nil
	case types.SchemaTypeBodyPathType:
		return node.PathValue(), nil
	case types.SchemaTypeBodyUrlType:
		return node.UrlValue(), nil

	case types.SchemaTypeBodyDatetimeType:
		dt := node.DatetimeValue()
		return time.Unix(dt.Seconds, int64(dt.Nanoseconds)).UTC().Format(time.RFC3339Nano), nil
	case types.SchemaTypeBodyDurationType:
		return map[string]any{
			"nanoseconds": strconv.FormatInt(node.DurationValue().Nanoseconds, 10),
		}, nil
	case types.SchemaTypeBodyQuantityType:
		q := node.QuantityValueNode()
		return map[string]any{
			"mantissa": strconv.FormatInt(q.Mantissa, 10),
			"scale":    q.Scale,
			"unit":     q.Unit,
		}, nil
	case types.SchemaTypeBodyTextType:
		payload := node.TextValue()
		out := map[string]any{"text": payload.Text}
		if payload.Language.IsSome() {
			out["language"] = payload.Language.Some()
		}
		return out, nil
	case types.SchemaTypeBodyBinaryType:
		payload := node.BinaryValue()
		out := map[string]any{"bytes": base64.RawURLEncoding.EncodeToString(payload.Bytes)}
		if payload.MimeType.IsSome() {
			out["mimeType"] = payload.MimeType.Some()
		}
		return out, nil

	case types.SchemaTypeBodyRecordType:
		fields := body.RecordType()
		values := node.RecordValue()
		out := make(map[string]any, len(fields))
		for i, f := range fields {
			rendered, err := u.render(f.Body, values[i], child(path, f.Name))
			if err != nil {
				return nil, err
			}
			out[f.Name] = rendered
		}
		return out, nil
	case types.SchemaTypeBodyVariantType:
		cases := body.VariantType()
		payload := node.VariantValue()
		declared := cases[payload.Case]
		if !payload.Payload.IsSome() {
			// A payload-less case is just its name.
			return declared.Name, nil
		}
		rendered, err := u.render(declared.Payload.Some(), payload.Payload.Some(), child(path, declared.Name))
		if err != nil {
			return nil, err
		}
		return map[string]any{declared.Name: rendered}, nil
	case types.SchemaTypeBodyEnumType:
		return body.EnumType()[node.EnumValue()], nil
	case types.SchemaTypeBodyFlagsType:
		declared := body.FlagsType()
		set := node.FlagsValue()
		out := make([]any, 0, len(declared))
		for i, on := range set {
			if on {
				out = append(out, declared[i])
			}
		}
		return out, nil
	case types.SchemaTypeBodyTupleType:
		elems := body.TupleType()
		values := node.TupleValue()
		out := make([]any, 0, len(values))
		for i, elem := range elems {
			rendered, err := u.render(elem, values[i], child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, rendered)
		}
		return out, nil
	case types.SchemaTypeBodyListType:
		elem := body.ListType()
		values := node.ListValue()
		out := make([]any, 0, len(values))
		for i, item := range values {
			rendered, err := u.render(elem, item, child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, rendered)
		}
		return out, nil
	case types.SchemaTypeBodyFixedListType:
		spec := body.FixedListType()
		values := node.FixedListValue()
		out := make([]any, 0, len(values))
		for i, item := range values {
			rendered, err := u.render(spec.Element, item, child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, rendered)
		}
		return out, nil
	case types.SchemaTypeBodyMapType:
		// A map is a list of pairs: its keys are not necessarily strings.
		spec := body.MapType()
		entries := node.MapValue()
		out := make([]any, 0, len(entries))
		for i, entry := range entries {
			k, err := u.render(spec.Key, entry.Key, child(path, fmt.Sprintf("[%d].key", i)))
			if err != nil {
				return nil, err
			}
			v, err := u.render(spec.Value, entry.Value, child(path, fmt.Sprintf("[%d].value", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, []any{k, v})
		}
		return out, nil
	case types.SchemaTypeBodyOptionType:
		inner := node.OptionValue()
		if !inner.IsSome() {
			return nil, nil
		}
		return u.render(body.OptionType(), inner.Some(), path)
	case types.SchemaTypeBodyResultType:
		spec := body.ResultType()
		payload := node.ResultValue()
		if payload.Tag() == types.ResultValuePayloadOkValue {
			return u.renderResultArm("ok", spec.Ok, payload.OkValue(), child(path, "ok"))
		}
		return u.renderResultArm("err", spec.Err, payload.ErrValue(), child(path, "err"))
	case types.SchemaTypeBodyUnionType:
		// A union is written as its branch body: the tag is recovered by the
		// branch's discriminator rule, not carried in the JSON.
		payload := node.UnionValue()
		for _, branch := range body.UnionType().Branches {
			if branch.Tag == payload.Tag {
				return u.render(branch.Body, payload.Body, child(path, payload.Tag))
			}
		}
		return nil, fmt.Errorf("%s: union tag %q is not a declared branch", pathOrRoot(path), payload.Tag)

	case types.SchemaTypeBodySecretType, types.SchemaTypeBodyQuotaTokenType,
		types.SchemaTypeBodyPermissionCardType:
		// These are host-held handles. A guest sees an opaque reference, never
		// the material, so there is nothing to render.
		return nil, fmt.Errorf("%s: host-managed capabilities cannot be rendered as JSON", pathOrRoot(path))
	case types.SchemaTypeBodyStreamType, types.SchemaTypeBodyFutureType:
		return nil, fmt.Errorf("%s: streams and futures have no JSON representation", pathOrRoot(path))
	default:
		return nil, fmt.Errorf("%s: unsupported schema type (tag %d)", pathOrRoot(path), body.Tag())
	}
}

func (u *unpacker) renderResultArm(arm string, declared, carried witTypes.Option[int32], path string) (any, error) {
	out := map[string]any{arm: nil}
	if declared.IsSome() && carried.IsSome() {
		rendered, err := u.render(declared.Some(), carried.Some(), path)
		if err != nil {
			return nil, err
		}
		out[arm] = rendered
	}
	return out, nil
}

// PackJSON is the inverse of [Ref.UnpackJSON]: it builds a value tree from
// canonical JSON and validates the result.
func (r Ref) PackJSON(value any) (types.SchemaValueTree, error) {
	p := &packer{ref: r}
	root, err := p.build(r.root, value, "")
	if err != nil {
		return types.SchemaValueTree{}, err
	}
	tree := types.SchemaValueTree{ValueNodes: p.nodes, Root: root}
	if err := r.Validate(tree); err != nil {
		return types.SchemaValueTree{}, err
	}
	return tree, nil
}

// PackJSONBytes is [Ref.PackJSON] over serialised JSON.
func (r Ref) PackJSONBytes(data []byte) (types.SchemaValueTree, error) {
	var value any
	decoder := json.NewDecoder(bytes.NewReader(data))
	// Numbers stay exact until the schema says how to read them.
	decoder.UseNumber()
	if err := decoder.Decode(&value); err != nil {
		return types.SchemaValueTree{}, fmt.Errorf("golem: invalid JSON: %w", err)
	}
	return r.PackJSON(value)
}

type packer struct {
	ref   Ref
	nodes []types.SchemaValueNode
}

func (p *packer) push(n types.SchemaValueNode) int32 {
	p.nodes = append(p.nodes, n)
	return int32(len(p.nodes) - 1)
}

func (p *packer) build(typeIdx int32, value any, path string) (int32, error) {
	body, _, err := p.ref.node(typeIdx)
	if err != nil {
		return 0, err
	}

	switch body.Tag() {
	case types.SchemaTypeBodyBoolType:
		b, ok := value.(bool)
		if !ok {
			return 0, typeErr(path, "boolean", value)
		}
		return p.push(types.MakeSchemaValueNodeBoolValue(b)), nil

	case types.SchemaTypeBodyS8Type:
		n, err := jsonInt(value, path, math.MinInt8, math.MaxInt8)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeS8Value(int8(n))), nil
	case types.SchemaTypeBodyS16Type:
		n, err := jsonInt(value, path, math.MinInt16, math.MaxInt16)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeS16Value(int16(n))), nil
	case types.SchemaTypeBodyS32Type:
		n, err := jsonInt(value, path, math.MinInt32, math.MaxInt32)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeS32Value(int32(n))), nil
	case types.SchemaTypeBodyS64Type:
		raw, ok := value.(string)
		if !ok {
			return 0, typeErr(path, "canonical integer string", value)
		}
		n, err := checkedIntegerString(raw, pathOrRoot(path))
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeS64Value(n)), nil

	case types.SchemaTypeBodyU8Type:
		n, err := jsonUint(value, path, math.MaxUint8)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeU8Value(uint8(n))), nil
	case types.SchemaTypeBodyU16Type:
		n, err := jsonUint(value, path, math.MaxUint16)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeU16Value(uint16(n))), nil
	case types.SchemaTypeBodyU32Type:
		n, err := jsonUint(value, path, math.MaxUint32)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeU32Value(uint32(n))), nil
	case types.SchemaTypeBodyU64Type:
		raw, ok := value.(string)
		if !ok {
			return 0, typeErr(path, "canonical integer string", value)
		}
		n, err := checkedUnsignedString(raw, pathOrRoot(path))
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeU64Value(n)), nil

	case types.SchemaTypeBodyF32Type:
		f, err := jsonFloat(value, path)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeF32Value(float32(f))), nil
	case types.SchemaTypeBodyF64Type:
		f, err := jsonFloat(value, path)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeF64Value(f)), nil

	case types.SchemaTypeBodyCharType:
		s, ok := value.(string)
		if !ok {
			return 0, typeErr(path, "string", value)
		}
		runes := []rune(s)
		if len(runes) != 1 {
			return 0, fmt.Errorf("%s: a char must be exactly one character, found %d", pathOrRoot(path), len(runes))
		}
		return p.push(types.MakeSchemaValueNodeCharValue(runes[0])), nil
	case types.SchemaTypeBodyStringType:
		s, ok := value.(string)
		if !ok {
			return 0, typeErr(path, "string", value)
		}
		return p.push(types.MakeSchemaValueNodeStringValue(s)), nil
	case types.SchemaTypeBodyPathType:
		s, ok := value.(string)
		if !ok || s == "" {
			return 0, typeErr(path, "non-empty string", value)
		}
		return p.push(types.MakeSchemaValueNodePathValue(s)), nil
	case types.SchemaTypeBodyUrlType:
		s, ok := value.(string)
		if !ok || s == "" {
			return 0, typeErr(path, "non-empty string", value)
		}
		return p.push(types.MakeSchemaValueNodeUrlValue(s)), nil

	case types.SchemaTypeBodyDatetimeType:
		s, ok := value.(string)
		if !ok {
			return 0, typeErr(path, "RFC 3339 timestamp", value)
		}
		t, err := time.Parse(time.RFC3339Nano, s)
		if err != nil {
			return 0, fmt.Errorf("%s: %w", pathOrRoot(path), err)
		}
		utc := t.UTC()
		return p.push(types.MakeSchemaValueNodeDatetimeValue(types.Datetime{
			Seconds:     utc.Unix(),
			Nanoseconds: uint32(utc.Nanosecond()),
		})), nil
	case types.SchemaTypeBodyDurationType:
		obj, ok := value.(map[string]any)
		if !ok {
			return 0, typeErr(path, `object with "nanoseconds"`, value)
		}
		raw, ok := obj["nanoseconds"].(string)
		if !ok {
			return 0, typeErr(child(path, "nanoseconds"), "canonical integer string", obj["nanoseconds"])
		}
		ns, err := checkedIntegerString(raw, child(path, "nanoseconds"))
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeDurationValue(types.DurationValuePayload{Nanoseconds: ns})), nil
	case types.SchemaTypeBodyQuantityType:
		obj, ok := value.(map[string]any)
		if !ok {
			return 0, typeErr(path, "quantity object", value)
		}
		rawMantissa, ok := obj["mantissa"].(string)
		if !ok {
			return 0, typeErr(child(path, "mantissa"), "canonical integer string", obj["mantissa"])
		}
		mantissa, err := checkedIntegerString(rawMantissa, child(path, "mantissa"))
		if err != nil {
			return 0, err
		}
		scale, err := jsonInt(obj["scale"], child(path, "scale"), math.MinInt32, math.MaxInt32)
		if err != nil {
			return 0, err
		}
		unit, ok := obj["unit"].(string)
		if !ok {
			return 0, typeErr(child(path, "unit"), "string", obj["unit"])
		}
		return p.push(types.MakeSchemaValueNodeQuantityValueNode(types.QuantityValue{
			Mantissa: mantissa, Scale: int32(scale), Unit: unit,
		})), nil
	case types.SchemaTypeBodyTextType:
		obj, ok := value.(map[string]any)
		if !ok {
			return 0, typeErr(path, "text object", value)
		}
		text, ok := obj["text"].(string)
		if !ok {
			return 0, typeErr(child(path, "text"), "string", obj["text"])
		}
		payload := types.TextValuePayload{Text: text, Language: witTypes.None[string]()}
		if lang, present := obj["language"]; present {
			s, ok := lang.(string)
			if !ok {
				return 0, typeErr(child(path, "language"), "string", lang)
			}
			payload.Language = witTypes.Some(s)
		}
		return p.push(types.MakeSchemaValueNodeTextValue(payload)), nil
	case types.SchemaTypeBodyBinaryType:
		obj, ok := value.(map[string]any)
		if !ok {
			return 0, typeErr(path, "binary object", value)
		}
		encoded, ok := obj["bytes"].(string)
		if !ok {
			return 0, typeErr(child(path, "bytes"), "base64url string", obj["bytes"])
		}
		raw, err := base64.RawURLEncoding.DecodeString(encoded)
		if err != nil {
			return 0, fmt.Errorf("%s: %w", child(path, "bytes"), err)
		}
		payload := types.BinaryValuePayload{Bytes: raw, MimeType: witTypes.None[string]()}
		if mime, present := obj["mimeType"]; present {
			s, ok := mime.(string)
			if !ok {
				return 0, typeErr(child(path, "mimeType"), "string", mime)
			}
			payload.MimeType = witTypes.Some(s)
		}
		return p.push(types.MakeSchemaValueNodeBinaryValue(payload)), nil

	case types.SchemaTypeBodyRecordType:
		obj, ok := value.(map[string]any)
		if !ok {
			return 0, typeErr(path, "object", value)
		}
		fields := body.RecordType()
		indices := make([]int32, 0, len(fields))
		for _, f := range fields {
			fieldValue, present := obj[f.Name]
			if !present {
				return 0, fmt.Errorf("%s: missing field %q", pathOrRoot(path), f.Name)
			}
			idx, err := p.build(f.Body, fieldValue, child(path, f.Name))
			if err != nil {
				return 0, err
			}
			indices = append(indices, idx)
		}
		for name := range obj {
			if !hasField(fields, name) {
				return 0, fmt.Errorf("%s: unknown field %q", pathOrRoot(path), name)
			}
		}
		return p.push(types.MakeSchemaValueNodeRecordValue(indices)), nil

	case types.SchemaTypeBodyVariantType:
		cases := body.VariantType()
		// A payload-less case is written as a bare string.
		if name, ok := value.(string); ok {
			for i, c := range cases {
				if c.Name == name {
					if c.Payload.IsSome() {
						return 0, fmt.Errorf("%s: case %q expects a payload", pathOrRoot(path), name)
					}
					return p.push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
						Case: uint32(i), Payload: witTypes.None[int32](),
					})), nil
				}
			}
			return 0, fmt.Errorf("%s: %q is not a declared case", pathOrRoot(path), name)
		}
		obj, ok := value.(map[string]any)
		if !ok || len(obj) != 1 {
			return 0, typeErr(path, "case name or single-key object", value)
		}
		for name, payload := range obj {
			for i, c := range cases {
				if c.Name != name {
					continue
				}
				if !c.Payload.IsSome() {
					return 0, fmt.Errorf("%s: case %q takes no payload", pathOrRoot(path), name)
				}
				idx, err := p.build(c.Payload.Some(), payload, child(path, name))
				if err != nil {
					return 0, err
				}
				return p.push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
					Case: uint32(i), Payload: witTypes.Some(idx),
				})), nil
			}
			return 0, fmt.Errorf("%s: %q is not a declared case", pathOrRoot(path), name)
		}
		return 0, typeErr(path, "variant", value)

	case types.SchemaTypeBodyEnumType:
		name, ok := value.(string)
		if !ok {
			return 0, typeErr(path, "enum case name", value)
		}
		for i, c := range body.EnumType() {
			if c == name {
				return p.push(types.MakeSchemaValueNodeEnumValue(uint32(i))), nil
			}
		}
		return 0, fmt.Errorf("%s: %q is not a declared enum case", pathOrRoot(path), name)

	case types.SchemaTypeBodyFlagsType:
		items, ok := value.([]any)
		if !ok {
			return 0, typeErr(path, "array of flag names", value)
		}
		declared := body.FlagsType()
		bits := make([]bool, len(declared))
		for _, item := range items {
			name, ok := item.(string)
			if !ok {
				return 0, typeErr(path, "flag name", item)
			}
			found := false
			for i, d := range declared {
				if d == name {
					bits[i] = true
					found = true
					break
				}
			}
			if !found {
				return 0, fmt.Errorf("%s: %q is not a declared flag", pathOrRoot(path), name)
			}
		}
		return p.push(types.MakeSchemaValueNodeFlagsValue(bits)), nil

	case types.SchemaTypeBodyTupleType:
		items, ok := value.([]any)
		if !ok {
			return 0, typeErr(path, "array", value)
		}
		elems := body.TupleType()
		if len(items) != len(elems) {
			return 0, fmt.Errorf("%s: tuple expects %d element(s), found %d", pathOrRoot(path), len(elems), len(items))
		}
		indices := make([]int32, 0, len(items))
		for i, elem := range elems {
			idx, err := p.build(elem, items[i], child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return 0, err
			}
			indices = append(indices, idx)
		}
		return p.push(types.MakeSchemaValueNodeTupleValue(indices)), nil

	case types.SchemaTypeBodyListType:
		items, ok := value.([]any)
		if !ok {
			return 0, typeErr(path, "array", value)
		}
		elem := body.ListType()
		indices := make([]int32, 0, len(items))
		for i, item := range items {
			idx, err := p.build(elem, item, child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return 0, err
			}
			indices = append(indices, idx)
		}
		return p.push(types.MakeSchemaValueNodeListValue(indices)), nil

	case types.SchemaTypeBodyFixedListType:
		items, ok := value.([]any)
		if !ok {
			return 0, typeErr(path, "array", value)
		}
		spec := body.FixedListType()
		if uint32(len(items)) != spec.Length {
			return 0, fmt.Errorf("%s: list expects %d element(s), found %d", pathOrRoot(path), spec.Length, len(items))
		}
		indices := make([]int32, 0, len(items))
		for i, item := range items {
			idx, err := p.build(spec.Element, item, child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return 0, err
			}
			indices = append(indices, idx)
		}
		return p.push(types.MakeSchemaValueNodeFixedListValue(indices)), nil

	case types.SchemaTypeBodyMapType:
		items, ok := value.([]any)
		if !ok {
			return 0, typeErr(path, "array of [key, value] pairs", value)
		}
		spec := body.MapType()
		entries := make([]types.MapEntry, 0, len(items))
		for i, item := range items {
			pair, ok := item.([]any)
			if !ok || len(pair) != 2 {
				return 0, typeErr(child(path, fmt.Sprintf("[%d]", i)), "[key, value] pair", item)
			}
			k, err := p.build(spec.Key, pair[0], child(path, fmt.Sprintf("[%d].key", i)))
			if err != nil {
				return 0, err
			}
			v, err := p.build(spec.Value, pair[1], child(path, fmt.Sprintf("[%d].value", i)))
			if err != nil {
				return 0, err
			}
			entries = append(entries, types.MapEntry{Key: k, Value: v})
		}
		return p.push(types.MakeSchemaValueNodeMapValue(entries)), nil

	case types.SchemaTypeBodyOptionType:
		if value == nil {
			return p.push(types.MakeSchemaValueNodeOptionValue(witTypes.None[int32]())), nil
		}
		idx, err := p.build(body.OptionType(), value, path)
		if err != nil {
			return 0, err
		}
		return p.push(types.MakeSchemaValueNodeOptionValue(witTypes.Some(idx))), nil

	case types.SchemaTypeBodyResultType:
		obj, ok := value.(map[string]any)
		if !ok || len(obj) != 1 {
			return 0, typeErr(path, `object with "ok" or "err"`, value)
		}
		spec := body.ResultType()
		if inner, present := obj["ok"]; present {
			payload, err := p.buildResultArm(spec.Ok, inner, child(path, "ok"))
			if err != nil {
				return 0, err
			}
			return p.push(types.MakeSchemaValueNodeResultValue(types.MakeResultValuePayloadOkValue(payload))), nil
		}
		if inner, present := obj["err"]; present {
			payload, err := p.buildResultArm(spec.Err, inner, child(path, "err"))
			if err != nil {
				return 0, err
			}
			return p.push(types.MakeSchemaValueNodeResultValue(types.MakeResultValuePayloadErrValue(payload))), nil
		}
		return 0, typeErr(path, `object with "ok" or "err"`, value)

	case types.SchemaTypeBodyUnionType:
		// The JSON carries no tag: each branch is tried in declaration order
		// and the first that accepts the value wins, which is what the
		// discriminator rules encode.
		branches := body.UnionType().Branches
		for _, branch := range branches {
			mark := len(p.nodes)
			idx, err := p.build(branch.Body, value, child(path, branch.Tag))
			if err == nil {
				return p.push(types.MakeSchemaValueNodeUnionValue(types.UnionValuePayload{
					Tag: branch.Tag, Body: idx,
				})), nil
			}
			p.nodes = p.nodes[:mark] // discard the partial branch attempt
		}
		return 0, fmt.Errorf("%s: value matches no declared union branch", pathOrRoot(path))

	case types.SchemaTypeBodySecretType, types.SchemaTypeBodyQuotaTokenType,
		types.SchemaTypeBodyPermissionCardType:
		return 0, fmt.Errorf("%s: host-managed capabilities cannot be built from JSON", pathOrRoot(path))
	case types.SchemaTypeBodyStreamType, types.SchemaTypeBodyFutureType:
		return 0, fmt.Errorf("%s: streams and futures cannot be built from JSON", pathOrRoot(path))
	default:
		return 0, fmt.Errorf("%s: unsupported schema type (tag %d)", pathOrRoot(path), body.Tag())
	}
}

func (p *packer) buildResultArm(declared witTypes.Option[int32], value any, path string) (witTypes.Option[int32], error) {
	if !declared.IsSome() {
		if value != nil {
			return witTypes.None[int32](), fmt.Errorf("%s: arm declares no payload", pathOrRoot(path))
		}
		return witTypes.None[int32](), nil
	}
	idx, err := p.build(declared.Some(), value, path)
	if err != nil {
		return witTypes.None[int32](), err
	}
	return witTypes.Some(idx), nil
}

func hasField(fields []types.NamedFieldType, name string) bool {
	for _, f := range fields {
		if f.Name == name {
			return true
		}
	}
	return false
}

// jsonInt reads a JSON number as a bounded signed integer. json.Number keeps
// the original text, so a value too large for the target is reported rather
// than silently rounded.
func jsonInt(value any, path string, min, max int64) (int64, error) {
	switch v := value.(type) {
	case json.Number:
		n, err := v.Int64()
		if err != nil {
			return 0, fmt.Errorf("%s: %q is not an integer", pathOrRoot(path), v.String())
		}
		return boundsCheck(n, min, max, path)
	case float64:
		if v != math.Trunc(v) {
			return 0, fmt.Errorf("%s: %v is not an integer", pathOrRoot(path), v)
		}
		return boundsCheck(int64(v), min, max, path)
	case int64:
		return boundsCheck(v, min, max, path)
	case int:
		return boundsCheck(int64(v), min, max, path)
	default:
		return 0, typeErr(path, "integer", value)
	}
}

func boundsCheck(n, min, max int64, path string) (int64, error) {
	if n < min || n > max {
		return 0, fmt.Errorf("%s: %d is outside the range [%d, %d]", pathOrRoot(path), n, min, max)
	}
	return n, nil
}

func jsonUint(value any, path string, max uint64) (uint64, error) {
	n, err := jsonInt(value, path, 0, math.MaxInt64)
	if err != nil {
		return 0, err
	}
	if uint64(n) > max {
		return 0, fmt.Errorf("%s: %d is outside the range [0, %d]", pathOrRoot(path), n, max)
	}
	return uint64(n), nil
}

func jsonFloat(value any, path string) (float64, error) {
	switch v := value.(type) {
	case json.Number:
		f, err := v.Float64()
		if err != nil {
			return 0, fmt.Errorf("%s: %q is not a number", pathOrRoot(path), v.String())
		}
		return f, nil
	case float64:
		return v, nil
	case int64:
		return float64(v), nil
	case int:
		return float64(v), nil
	default:
		return 0, typeErr(path, "number", value)
	}
}

func typeErr(path, want string, got any) error {
	return fmt.Errorf("%s: expected %s, found %s", pathOrRoot(path), want, jsonKindName(got))
}

func jsonKindName(v any) string {
	switch t := v.(type) {
	case nil:
		return "null"
	case bool:
		return "boolean"
	case string:
		return "string"
	case json.Number:
		return "number " + t.String()
	case float64, int64, int:
		return "number"
	case []any:
		return "array"
	case map[string]any:
		return "object"
	default:
		return fmt.Sprintf("%T", v)
	}
}

func pathOrRoot(path string) string {
	if path == "" {
		return "value"
	}
	return path
}

// BoolParameterNode marks a parameter whose type is fixed as boolean rather
// than named by a node in the graph. A tool flag is the case that needs it: the
// WIT gives a flag no type index because its type can only be bool, so a graph
// carrying flags need not contain a bool node at all.
const BoolParameterNode int32 = -1

// Parameter is one field of a named parameter list: its wire name and the type
// node in the graph that its value is read against, or [BoolParameterNode].
type Parameter struct {
	Name string
	Node int32
}

// BoolParameter describes a parameter whose type is fixed as boolean.
func BoolParameter(name string) Parameter {
	return Parameter{Name: name, Node: BoolParameterNode}
}

// PackParameters builds the value tree an invocation carries: a record whose
// fields are the parameters in declaration order, each packed against its own
// type node. The caller supplies the arguments by name, which is how a
// reflective client works without knowing the target's language types.
//
// A missing argument is an error, and so is one the parameter list does not
// declare, so a caller learns about a typo here rather than at the target.
func (r Ref) PackParameters(params []Parameter, args map[string]any) (types.SchemaValueTree, error) {
	var issues []Issue
	for name := range args {
		if !hasParameter(params, name) {
			issues = append(issues, Issue{Path: name, Message: "unexpected argument"})
		}
	}

	p := &packer{ref: r}
	idxs := make([]int32, 0, len(params))
	for _, param := range params {
		value, given := args[param.Name]
		if !given {
			issues = append(issues, Issue{Path: param.Name, Message: "missing argument"})
			continue
		}
		var idx int32
		var err error
		if param.Node == BoolParameterNode {
			idx, err = p.buildBool(value, param.Name)
		} else {
			idx, err = p.build(param.Node, value, param.Name)
		}
		if err != nil {
			var ve *ValidationError
			if errors.As(err, &ve) {
				issues = append(issues, ve.Issues...)
				continue
			}
			issues = append(issues, Issue{Path: param.Name, Message: err.Error()})
			continue
		}
		idxs = append(idxs, idx)
	}
	if len(issues) > 0 {
		return types.SchemaValueTree{}, &ValidationError{Issues: issues}
	}

	root := p.push(types.MakeSchemaValueNodeRecordValue(idxs))
	tree := types.SchemaValueTree{ValueNodes: p.nodes, Root: root}
	// Each parameter is validated against its own node, since the record is an
	// invocation envelope rather than a type in the graph. A fixed-type
	// parameter was already checked when it was built.
	for i, param := range params {
		if param.Node == BoolParameterNode {
			continue
		}
		if err := r.WithRoot(param.Node).Validate(types.SchemaValueTree{
			ValueNodes: tree.ValueNodes, Root: idxs[i],
		}); err != nil {
			return types.SchemaValueTree{}, err
		}
	}
	return tree, nil
}

// UnpackParameters is the inverse of [Ref.PackParameters], reading an
// invocation's parameter record back into named arguments.
func (r Ref) UnpackParameters(params []Parameter, tree types.SchemaValueTree) (map[string]any, error) {
	root, err := nodeAt(tree, tree.Root)
	if err != nil {
		return nil, err
	}
	if root.Tag() != types.SchemaValueNodeRecordValue {
		return nil, fmt.Errorf("golem: expected a record at the root of the parameter list")
	}
	idxs := root.RecordValue()
	if len(idxs) != len(params) {
		return nil, fmt.Errorf("golem: parameter list has %d value(s), want %d", len(idxs), len(params))
	}
	out := make(map[string]any, len(params))
	for i, param := range params {
		if param.Node == BoolParameterNode {
			node, err := nodeAt(tree, idxs[i])
			if err != nil {
				return nil, err
			}
			if node.Tag() != types.SchemaValueNodeBoolValue {
				return nil, fmt.Errorf("golem: parameter %q: expected a boolean", param.Name)
			}
			out[param.Name] = node.BoolValue()
			continue
		}
		value, err := r.WithRoot(param.Node).UnpackJSON(types.SchemaValueTree{
			ValueNodes: tree.ValueNodes, Root: idxs[i],
		})
		if err != nil {
			return nil, fmt.Errorf("golem: parameter %q: %w", param.Name, err)
		}
		out[param.Name] = value
	}
	return out, nil
}

// ParametersJSONSchema renders a parameter list as a JSON Schema object, which
// is the shape a model or a form is given to fill in.
func (r Ref) ParametersJSONSchema(params []Parameter, includeDraftMarker bool) (any, error) {
	props := obj{}
	required := make([]string, 0, len(params))
	for _, param := range params {
		if param.Node == BoolParameterNode {
			props[param.Name] = obj{"type": "boolean"}
			required = append(required, param.Name)
			continue
		}
		rendered, err := r.renderSchema(param.Node)
		if err != nil {
			return nil, err
		}
		props[param.Name] = rendered
		optional, err := r.resolvesToOption(param.Node)
		if err != nil {
			return nil, err
		}
		if !optional {
			required = append(required, param.Name)
		}
	}
	out := obj{
		"type":                 "object",
		"properties":           props,
		"required":             requiredList(required),
		"additionalProperties": false,
	}
	if includeDraftMarker {
		out["$schema"] = jsonSchemaDraft
	}
	if len(r.graph.Defs) > 0 {
		defs := obj{}
		for _, def := range r.graph.Defs {
			rendered, err := r.renderSchema(def.Body)
			if err != nil {
				return nil, err
			}
			if _, has := rendered["title"]; !has && def.Name.IsSome() {
				rendered["title"] = def.Name.Some()
			}
			defs[def.Id] = rendered
		}
		out["$defs"] = defs
	}
	return out, nil
}

func hasParameter(params []Parameter, name string) bool {
	for _, p := range params {
		if p.Name == name {
			return true
		}
	}
	return false
}

// nodeAt reads a value node, reporting an out-of-range index rather than
// panicking.
func nodeAt(tree types.SchemaValueTree, idx int32) (types.SchemaValueNode, error) {
	if idx < 0 || int(idx) >= len(tree.ValueNodes) {
		return types.SchemaValueNode{}, fmt.Errorf(
			"golem: value node index %d out of range (%d nodes)", idx, len(tree.ValueNodes))
	}
	return tree.ValueNodes[idx], nil
}

// buildBool packs a parameter whose type is fixed as boolean.
func (p *packer) buildBool(value any, path string) (int32, error) {
	b, ok := value.(bool)
	if !ok {
		return 0, &ValidationError{Issues: []Issue{{Path: path, Message: "expected a boolean"}}}
	}
	return p.push(types.MakeSchemaValueNodeBoolValue(b)), nil
}
