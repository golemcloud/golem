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
	"fmt"
	"math"
	"regexp"
	"strconv"
	"time"
)

// Canonical JSON.
//
// This is the one implementation in Go, and it follows the host exactly. The
// contract is written down in golem-skills/skills/common/golem-agent-reflection
// and implemented by golem-schema/src/schema/render/json_value.rs:
//
//   - integers through 32 bits are JSON numbers;
//   - s64, u64, duration nanoseconds and quantity mantissas are canonical
//     base-10 strings, because a JSON number cannot carry the full range;
//   - a duration is {"nanoseconds": "…"}, a quantity is
//     {"mantissa": "…", "scale": n, "unit": "…"};
//   - leading zeroes, a leading +, and -0 are all invalid;
//   - a variant case with no payload is its bare name, one with a payload is
//     {name: value};
//   - flags are the array of selected names;
//   - a map is [[k, v], …], since keys need not be strings;
//   - an absent option is null;
//   - a result is {"ok": …} or {"err": …};
//   - binary is base64url with no padding.
//
// Diverging from any of this is not a stylistic choice: GOL-653 records what
// happened when two other SDKs did.

// UnpackJSON renders a value as canonical JSON, using ordinary Go values.
func (r Ref) UnpackJSON(v SchemaValue) (any, error) {
	return (&unpacker{ref: r}).render(r.typ, v, "")
}

type unpacker struct{ ref Ref }

func (u *unpacker) render(t SchemaType, v SchemaValue, path string) (any, error) {
	body, err := u.ref.At(t).body()
	if err != nil {
		return nil, err
	}

	switch b := body.(type) {
	case BoolType:
		n, err := expect[BoolValue](v, path, "bool")
		return n.Value, err
	case S8Type:
		n, err := expect[S8Value](v, path, "s8")
		return int64(n.Value), err
	case S16Type:
		n, err := expect[S16Value](v, path, "s16")
		return int64(n.Value), err
	case S32Type:
		n, err := expect[S32Value](v, path, "s32")
		return int64(n.Value), err
	case S64Type:
		// Wide integers travel as strings; see the contract above.
		n, err := expect[S64Value](v, path, "s64")
		if err != nil {
			return nil, err
		}
		return strconv.FormatInt(n.Value, 10), nil
	case U8Type:
		n, err := expect[U8Value](v, path, "u8")
		return uint64(n.Value), err
	case U16Type:
		n, err := expect[U16Value](v, path, "u16")
		return uint64(n.Value), err
	case U32Type:
		n, err := expect[U32Value](v, path, "u32")
		return uint64(n.Value), err
	case U64Type:
		n, err := expect[U64Value](v, path, "u64")
		if err != nil {
			return nil, err
		}
		return strconv.FormatUint(n.Value, 10), nil
	case F32Type:
		n, err := expect[F32Value](v, path, "f32")
		return float64(n.Value), err
	case F64Type:
		n, err := expect[F64Value](v, path, "f64")
		return n.Value, err
	case CharType:
		n, err := expect[CharValue](v, path, "char")
		return string(n.Value), err
	case StringType:
		n, err := expect[StringValue](v, path, "string")
		return n.Value, err
	case PathType:
		n, err := expect[PathValue](v, path, "path")
		return n.Value, err
	case UrlType:
		n, err := expect[UrlValue](v, path, "url")
		return n.Value, err

	case DatetimeType:
		n, err := expect[DatetimeValue](v, path, "datetime")
		if err != nil {
			return nil, err
		}
		return time.Unix(n.Seconds, int64(n.Nanoseconds)).UTC().Format(time.RFC3339Nano), nil
	case DurationType:
		n, err := expect[DurationValue](v, path, "duration")
		if err != nil {
			return nil, err
		}
		return map[string]any{"nanoseconds": strconv.FormatInt(n.Nanoseconds, 10)}, nil
	case QuantityType:
		n, err := expect[QuantityValueNode](v, path, "quantity")
		if err != nil {
			return nil, err
		}
		return map[string]any{
			"mantissa": strconv.FormatInt(n.Value.Mantissa, 10),
			"scale":    n.Value.Scale,
			"unit":     n.Value.Unit,
		}, nil
	case TextType:
		n, err := expect[TextValue](v, path, "text")
		if err != nil {
			return nil, err
		}
		out := map[string]any{"text": n.Text}
		if n.Language != nil {
			out["language"] = *n.Language
		}
		return out, nil
	case BinaryType:
		n, err := expect[BinaryValue](v, path, "binary")
		if err != nil {
			return nil, err
		}
		out := map[string]any{"bytes": base64.RawURLEncoding.EncodeToString(n.Bytes)}
		if n.MimeType != nil {
			out["mimeType"] = *n.MimeType
		}
		return out, nil

	case RecordType:
		n, err := expect[RecordValue](v, path, "record")
		if err != nil {
			return nil, err
		}
		if len(n.Fields) != len(b.Fields) {
			return nil, fmt.Errorf("%s: record has %d field(s), want %d",
				pathOrRoot(path), len(n.Fields), len(b.Fields))
		}
		out := make(map[string]any, len(b.Fields))
		for i, f := range b.Fields {
			rendered, err := u.render(f.Body, n.Fields[i], child(path, f.Name))
			if err != nil {
				return nil, err
			}
			out[f.Name] = rendered
		}
		return out, nil

	case VariantType:
		n, err := expect[VariantValue](v, path, "variant")
		if err != nil {
			return nil, err
		}
		if int(n.Case) >= len(b.Cases) {
			return nil, fmt.Errorf("%s: variant case %d is not declared", pathOrRoot(path), n.Case)
		}
		declared := b.Cases[n.Case]
		if n.Payload == nil {
			// A payload-less case is just its name.
			return declared.Name, nil
		}
		if declared.Payload == nil {
			return nil, fmt.Errorf("%s: variant case %q carries a payload but declares none",
				pathOrRoot(path), declared.Name)
		}
		rendered, err := u.render(*declared.Payload, *n.Payload, child(path, declared.Name))
		if err != nil {
			return nil, err
		}
		return map[string]any{declared.Name: rendered}, nil

	case EnumType:
		n, err := expect[EnumValue](v, path, "enum")
		if err != nil {
			return nil, err
		}
		if int(n.Case) >= len(b.Cases) {
			return nil, fmt.Errorf("%s: enum case %d is not declared", pathOrRoot(path), n.Case)
		}
		return b.Cases[n.Case], nil

	case FlagsType:
		n, err := expect[FlagsValue](v, path, "flags")
		if err != nil {
			return nil, err
		}
		if len(n.Set) != len(b.Flags) {
			return nil, fmt.Errorf("%s: flags value has %d entries, want %d",
				pathOrRoot(path), len(n.Set), len(b.Flags))
		}
		out := make([]any, 0, len(b.Flags))
		for i, on := range n.Set {
			if on {
				out = append(out, b.Flags[i])
			}
		}
		return out, nil

	case TupleType:
		n, err := expect[TupleValue](v, path, "tuple")
		if err != nil {
			return nil, err
		}
		if len(n.Elements) != len(b.Elements) {
			return nil, fmt.Errorf("%s: tuple has %d element(s), want %d",
				pathOrRoot(path), len(n.Elements), len(b.Elements))
		}
		out := make([]any, 0, len(b.Elements))
		for i, elem := range b.Elements {
			rendered, err := u.render(elem, n.Elements[i], child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, rendered)
		}
		return out, nil

	case ListType:
		n, err := expect[ListValue](v, path, "list")
		if err != nil {
			return nil, err
		}
		return u.renderItems(b.Element, n.Items, path)
	case FixedListType:
		n, err := expect[FixedListValue](v, path, "fixed list")
		if err != nil {
			return nil, err
		}
		if uint32(len(n.Items)) != b.Length {
			return nil, fmt.Errorf("%s: fixed list has %d item(s), want %d",
				pathOrRoot(path), len(n.Items), b.Length)
		}
		return u.renderItems(b.Element, n.Items, path)

	case MapType:
		// A map is a list of pairs: its keys are not necessarily strings.
		n, err := expect[MapValue](v, path, "map")
		if err != nil {
			return nil, err
		}
		out := make([]any, 0, len(n.Entries))
		for i, entry := range n.Entries {
			k, err := u.render(b.Key, entry.Key, child(path, fmt.Sprintf("[%d].key", i)))
			if err != nil {
				return nil, err
			}
			val, err := u.render(b.Value, entry.Value, child(path, fmt.Sprintf("[%d].value", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, []any{k, val})
		}
		return out, nil

	case OptionType:
		n, err := expect[OptionValue](v, path, "option")
		if err != nil {
			return nil, err
		}
		if n.Value == nil {
			return nil, nil
		}
		return u.render(b.Inner, *n.Value, path)

	case ResultType:
		n, err := expect[ResultValue](v, path, "result")
		if err != nil {
			return nil, err
		}
		arm, declared := "ok", b.Ok
		if n.IsErr {
			arm, declared = "err", b.Err
		}
		out := map[string]any{arm: nil}
		if declared != nil && n.Value != nil {
			rendered, err := u.render(*declared, *n.Value, child(path, arm))
			if err != nil {
				return nil, err
			}
			out[arm] = rendered
		}
		return out, nil

	case UnionType:
		// A union is written as its branch body: the tag is recovered by the
		// branch's discriminator rule, not carried in the JSON.
		n, err := expect[UnionValue](v, path, "union")
		if err != nil {
			return nil, err
		}
		for _, branch := range b.Branches {
			if branch.Tag == n.Tag {
				return u.render(branch.Body, n.Body, child(path, n.Tag))
			}
		}
		return nil, fmt.Errorf("%s: union tag %q is not a declared branch", pathOrRoot(path), n.Tag)

	case SecretType, QuotaTokenType, PermissionCardType:
		// These are host-held handles. A caller sees an opaque reference, never
		// the material, so there is nothing to render.
		return nil, fmt.Errorf("%s: host-managed capabilities cannot be rendered as JSON", pathOrRoot(path))
	case StreamType, FutureType:
		return nil, fmt.Errorf("%s: streams and futures have no JSON representation", pathOrRoot(path))
	}
	return nil, fmt.Errorf("%s: unsupported schema type %T", pathOrRoot(path), body)
}

func (u *unpacker) renderItems(elem SchemaType, items []SchemaValue, path string) (any, error) {
	out := make([]any, 0, len(items))
	for i, item := range items {
		rendered, err := u.render(elem, item, child(path, fmt.Sprintf("[%d]", i)))
		if err != nil {
			return nil, err
		}
		out = append(out, rendered)
	}
	return out, nil
}

// expect narrows a value to the case its type calls for. A mismatch here means
// the value and the schema disagree, which is worth reporting precisely rather
// than rendering something plausible.
func expect[T SchemaValue](v SchemaValue, path, want string) (T, error) {
	typed, ok := v.(T)
	if !ok {
		var zero T
		return zero, fmt.Errorf("%s: expected a %s value, got %T", pathOrRoot(path), want, v)
	}
	return typed, nil
}

// child extends a path for an error message.
func child(path, seg string) string {
	if path == "" {
		return seg
	}
	if len(seg) > 0 && seg[0] == '[' {
		return path + seg
	}
	return path + "." + seg
}

// pathOrRoot names the position in an error message.
func pathOrRoot(path string) string {
	if path == "" {
		return "value"
	}
	return path
}

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

// PackJSON is the inverse of [Ref.UnpackJSON]: it builds a value from canonical
// JSON.
func (r Ref) PackJSON(value any) (SchemaValue, error) {
	return (&packer{ref: r}).build(r.typ, value, "")
}

// PackJSONBytes is [Ref.PackJSON] over serialised JSON.
func (r Ref) PackJSONBytes(data []byte) (SchemaValue, error) {
	var value any
	dec := json.NewDecoder(bytes.NewReader(data))
	// Numbers stay exact until the schema says how to read them.
	dec.UseNumber()
	if err := dec.Decode(&value); err != nil {
		return nil, fmt.Errorf("golem: invalid JSON: %w", err)
	}
	return r.PackJSON(value)
}

// UnpackJSONBytes is [Ref.UnpackJSON] followed by serialisation.
func (r Ref) UnpackJSONBytes(v SchemaValue) ([]byte, error) {
	rendered, err := r.UnpackJSON(v)
	if err != nil {
		return nil, err
	}
	return json.Marshal(rendered)
}

type packer struct{ ref Ref }

func (p *packer) build(t SchemaType, value any, path string) (SchemaValue, error) {
	body, err := p.ref.At(t).body()
	if err != nil {
		return nil, err
	}

	switch b := body.(type) {
	case BoolType:
		v, ok := value.(bool)
		if !ok {
			return nil, typeErr(path, "boolean", value)
		}
		return BoolValue{Value: v}, nil

	case S8Type:
		n, err := jsonInt(value, path, math.MinInt8, math.MaxInt8)
		return S8Value{Value: int8(n)}, err
	case S16Type:
		n, err := jsonInt(value, path, math.MinInt16, math.MaxInt16)
		return S16Value{Value: int16(n)}, err
	case S32Type:
		n, err := jsonInt(value, path, math.MinInt32, math.MaxInt32)
		return S32Value{Value: int32(n)}, err
	case S64Type:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "canonical integer string", value)
		}
		n, err := checkedIntegerString(raw, pathOrRoot(path))
		return S64Value{Value: n}, err

	case U8Type:
		n, err := jsonUint(value, path, math.MaxUint8)
		return U8Value{Value: uint8(n)}, err
	case U16Type:
		n, err := jsonUint(value, path, math.MaxUint16)
		return U16Value{Value: uint16(n)}, err
	case U32Type:
		n, err := jsonUint(value, path, math.MaxUint32)
		return U32Value{Value: uint32(n)}, err
	case U64Type:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "canonical integer string", value)
		}
		n, err := checkedUnsignedString(raw, pathOrRoot(path))
		return U64Value{Value: n}, err

	case F32Type:
		f, err := jsonFloat(value, path)
		return F32Value{Value: float32(f)}, err
	case F64Type:
		f, err := jsonFloat(value, path)
		return F64Value{Value: f}, err

	case CharType:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "single-character string", value)
		}
		runes := []rune(raw)
		if len(runes) != 1 {
			return nil, fmt.Errorf("%s: expected exactly one character, found %d", pathOrRoot(path), len(runes))
		}
		return CharValue{Value: runes[0]}, nil
	case StringType:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "string", value)
		}
		return StringValue{Value: raw}, nil
	case PathType:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "string", value)
		}
		return PathValue{Value: raw}, nil
	case UrlType:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "string", value)
		}
		return UrlValue{Value: raw}, nil

	case DatetimeType:
		raw, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "RFC 3339 timestamp", value)
		}
		ts, err := time.Parse(time.RFC3339Nano, raw)
		if err != nil {
			return nil, fmt.Errorf("%s: %q is not an RFC 3339 timestamp", pathOrRoot(path), raw)
		}
		return DatetimeValue{Seconds: ts.Unix(), Nanoseconds: uint32(ts.Nanosecond())}, nil

	case DurationType:
		obj, ok := value.(map[string]any)
		if !ok {
			return nil, typeErr(path, `an object with "nanoseconds"`, value)
		}
		raw, ok := obj["nanoseconds"].(string)
		if !ok {
			return nil, typeErr(child(path, "nanoseconds"), "canonical integer string", obj["nanoseconds"])
		}
		n, err := checkedIntegerString(raw, pathOrRoot(child(path, "nanoseconds")))
		return DurationValue{Nanoseconds: n}, err

	case QuantityType:
		obj, ok := value.(map[string]any)
		if !ok {
			return nil, typeErr(path, "a quantity object", value)
		}
		rawMantissa, ok := obj["mantissa"].(string)
		if !ok {
			return nil, typeErr(child(path, "mantissa"), "canonical integer string", obj["mantissa"])
		}
		mantissa, err := checkedIntegerString(rawMantissa, pathOrRoot(child(path, "mantissa")))
		if err != nil {
			return nil, err
		}
		scale, err := jsonInt(obj["scale"], child(path, "scale"), math.MinInt32, math.MaxInt32)
		if err != nil {
			return nil, err
		}
		unit, ok := obj["unit"].(string)
		if !ok {
			return nil, typeErr(child(path, "unit"), "string", obj["unit"])
		}
		return QuantityValueNode{Value: QuantityValue{
			Mantissa: mantissa, Scale: int32(scale), Unit: unit,
		}}, nil

	case TextType:
		obj, ok := value.(map[string]any)
		if !ok {
			return nil, typeErr(path, `an object with "text"`, value)
		}
		text, ok := obj["text"].(string)
		if !ok {
			return nil, typeErr(child(path, "text"), "string", obj["text"])
		}
		out := TextValue{Text: text}
		if lang, present := obj["language"]; present && lang != nil {
			s, ok := lang.(string)
			if !ok {
				return nil, typeErr(child(path, "language"), "string", lang)
			}
			out.Language = &s
		}
		return out, nil

	case BinaryType:
		obj, ok := value.(map[string]any)
		if !ok {
			return nil, typeErr(path, `an object with "bytes"`, value)
		}
		encoded, ok := obj["bytes"].(string)
		if !ok {
			return nil, typeErr(child(path, "bytes"), "base64url string", obj["bytes"])
		}
		raw, err := base64.RawURLEncoding.DecodeString(encoded)
		if err != nil {
			return nil, fmt.Errorf("%s: not base64url without padding", pathOrRoot(child(path, "bytes")))
		}
		out := BinaryValue{Bytes: raw}
		if mime, present := obj["mimeType"]; present && mime != nil {
			s, ok := mime.(string)
			if !ok {
				return nil, typeErr(child(path, "mimeType"), "string", mime)
			}
			out.MimeType = &s
		}
		return out, nil

	case RecordType:
		obj, ok := value.(map[string]any)
		if !ok {
			return nil, typeErr(path, "object", value)
		}
		for key := range obj {
			if !hasField(b.Fields, key) {
				return nil, fmt.Errorf("%s: unknown field %q", pathOrRoot(path), key)
			}
		}
		fields := make([]SchemaValue, 0, len(b.Fields))
		for _, f := range b.Fields {
			raw, present := obj[f.Name]
			if !present {
				return nil, fmt.Errorf("%s: missing field %q", pathOrRoot(path), f.Name)
			}
			built, err := p.build(f.Body, raw, child(path, f.Name))
			if err != nil {
				return nil, err
			}
			fields = append(fields, built)
		}
		return RecordValue{Fields: fields}, nil

	case VariantType:
		// A payload-less case is its bare name; one with a payload is a
		// single-key object.
		if name, ok := value.(string); ok {
			for i, c := range b.Cases {
				if c.Name == name {
					if c.Payload != nil {
						return nil, fmt.Errorf("%s: case %q needs a payload", pathOrRoot(path), name)
					}
					return VariantValue{Case: uint32(i)}, nil
				}
			}
			return nil, fmt.Errorf("%s: %q is not a declared case", pathOrRoot(path), name)
		}
		obj, ok := value.(map[string]any)
		if !ok || len(obj) != 1 {
			return nil, typeErr(path, "a case name or a single-key object", value)
		}
		for name, raw := range obj {
			for i, c := range b.Cases {
				if c.Name != name {
					continue
				}
				if c.Payload == nil {
					return nil, fmt.Errorf("%s: case %q carries no payload", pathOrRoot(path), name)
				}
				built, err := p.build(*c.Payload, raw, child(path, name))
				if err != nil {
					return nil, err
				}
				return VariantValue{Case: uint32(i), Payload: &built}, nil
			}
			return nil, fmt.Errorf("%s: %q is not a declared case", pathOrRoot(path), name)
		}
		return nil, typeErr(path, "a case name or a single-key object", value)

	case EnumType:
		name, ok := value.(string)
		if !ok {
			return nil, typeErr(path, "string", value)
		}
		for i, c := range b.Cases {
			if c == name {
				return EnumValue{Case: uint32(i)}, nil
			}
		}
		return nil, fmt.Errorf("%s: %q is not a declared case", pathOrRoot(path), name)

	case FlagsType:
		items, ok := value.([]any)
		if !ok {
			return nil, typeErr(path, "array of flag names", value)
		}
		set := make([]bool, len(b.Flags))
		for _, item := range items {
			name, ok := item.(string)
			if !ok {
				return nil, typeErr(path, "array of flag names", item)
			}
			found := false
			for i, declared := range b.Flags {
				if declared == name {
					set[i], found = true, true
					break
				}
			}
			if !found {
				return nil, fmt.Errorf("%s: %q is not a declared flag", pathOrRoot(path), name)
			}
		}
		return FlagsValue{Set: set}, nil

	case TupleType:
		items, ok := value.([]any)
		if !ok {
			return nil, typeErr(path, "array", value)
		}
		if len(items) != len(b.Elements) {
			return nil, fmt.Errorf("%s: expected %d element(s), found %d",
				pathOrRoot(path), len(b.Elements), len(items))
		}
		out := make([]SchemaValue, 0, len(items))
		for i, elem := range b.Elements {
			built, err := p.build(elem, items[i], child(path, fmt.Sprintf("[%d]", i)))
			if err != nil {
				return nil, err
			}
			out = append(out, built)
		}
		return TupleValue{Elements: out}, nil

	case ListType:
		items, err := p.buildItems(b.Element, value, path)
		if err != nil {
			return nil, err
		}
		return ListValue{Items: items}, nil

	case FixedListType:
		items, err := p.buildItems(b.Element, value, path)
		if err != nil {
			return nil, err
		}
		if uint32(len(items)) != b.Length {
			return nil, fmt.Errorf("%s: expected %d item(s), found %d",
				pathOrRoot(path), b.Length, len(items))
		}
		return FixedListValue{Items: items}, nil

	case MapType:
		pairs, ok := value.([]any)
		if !ok {
			return nil, typeErr(path, "array of [key, value] pairs", value)
		}
		entries := make([]MapEntry, 0, len(pairs))
		for i, raw := range pairs {
			pair, ok := raw.([]any)
			if !ok || len(pair) != 2 {
				return nil, typeErr(child(path, fmt.Sprintf("[%d]", i)), "[key, value] pair", raw)
			}
			k, err := p.build(b.Key, pair[0], child(path, fmt.Sprintf("[%d].key", i)))
			if err != nil {
				return nil, err
			}
			v, err := p.build(b.Value, pair[1], child(path, fmt.Sprintf("[%d].value", i)))
			if err != nil {
				return nil, err
			}
			entries = append(entries, MapEntry{Key: k, Value: v})
		}
		return MapValue{Entries: entries}, nil

	case OptionType:
		if value == nil {
			return OptionValue{}, nil
		}
		inner, err := p.build(b.Inner, value, path)
		if err != nil {
			return nil, err
		}
		return OptionValue{Value: &inner}, nil

	case ResultType:
		obj, ok := value.(map[string]any)
		if !ok || len(obj) != 1 {
			return nil, typeErr(path, `{"ok": ...} or {"err": ...}`, value)
		}
		if raw, present := obj["ok"]; present {
			inner, err := p.buildResultArm(b.Ok, raw, child(path, "ok"))
			if err != nil {
				return nil, err
			}
			return ResultValue{Value: inner}, nil
		}
		if raw, present := obj["err"]; present {
			inner, err := p.buildResultArm(b.Err, raw, child(path, "err"))
			if err != nil {
				return nil, err
			}
			return ResultValue{IsErr: true, Value: inner}, nil
		}
		return nil, typeErr(path, `{"ok": ...} or {"err": ...}`, value)

	case UnionType:
		// The tag is not carried in the JSON, so branches are tried in
		// declaration order and the first that fits wins. A failed attempt
		// leaves nothing behind, since building returns a value rather than
		// appending to a pool.
		for _, branch := range b.Branches {
			if built, err := p.build(branch.Body, value, child(path, branch.Tag)); err == nil {
				return UnionValue{Tag: branch.Tag, Body: built}, nil
			}
		}
		return nil, fmt.Errorf("%s: value matches no declared union branch", pathOrRoot(path))

	case SecretType, QuotaTokenType, PermissionCardType:
		return nil, fmt.Errorf("%s: host-managed capabilities cannot be built from JSON", pathOrRoot(path))
	case StreamType, FutureType:
		return nil, fmt.Errorf("%s: streams and futures cannot be built from JSON", pathOrRoot(path))
	}
	return nil, fmt.Errorf("%s: unsupported schema type %T", pathOrRoot(path), body)
}

func (p *packer) buildItems(elem SchemaType, value any, path string) ([]SchemaValue, error) {
	items, ok := value.([]any)
	if !ok {
		return nil, typeErr(path, "array", value)
	}
	out := make([]SchemaValue, 0, len(items))
	for i, item := range items {
		built, err := p.build(elem, item, child(path, fmt.Sprintf("[%d]", i)))
		if err != nil {
			return nil, err
		}
		out = append(out, built)
	}
	return out, nil
}

func (p *packer) buildResultArm(declared *SchemaType, value any, path string) (*SchemaValue, error) {
	if declared == nil {
		if value != nil {
			return nil, fmt.Errorf("%s: arm declares no payload", pathOrRoot(path))
		}
		return nil, nil
	}
	built, err := p.build(*declared, value, path)
	if err != nil {
		return nil, err
	}
	return &built, nil
}

func hasField(fields []NamedField, name string) bool {
	for _, f := range fields {
		if f.Name == name {
			return true
		}
	}
	return false
}

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
