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
	"math"
	"regexp"
	"strconv"
	"strings"
)

// JSON Schema rendering.
//
// The document describes the canonical JSON above, so the two must agree: a
// reader told "integer" would send a JSON number, which the host refuses for
// s64 and u64. Wide integers are therefore declared as strings with a pattern,
// a duration as an object with a required nanoseconds string, and a quantity as
// {mantissa, scale, unit}.
//
// Union branches are inlined under oneOf with their discriminator as an allOf
// condition, rather than lifted into synthesised $defs entries. That is the
// guest-side convention the TypeScript SDK also follows; the host's own
// renderer synthesises per-branch definitions instead, and both are valid
// JSON Schema for the same values.

// jsonSchemaDraft is the dialect every rendered document declares.
const jsonSchemaDraft = "https://json-schema.org/draft/2020-12/schema"

// obj is the rendered form of one node. Marshalling sorts the keys, so the
// output is byte-stable.
type obj = map[string]any

// ToJSONSchema renders the type as a JSON Schema document. includeDraftMarker
// adds the $schema dialect declaration, which belongs on a standalone document
// but is usually stripped when the schema is embedded.
func (r Ref) ToJSONSchema(includeDraftMarker bool) (any, error) {
	root, err := r.renderSchema(r.typ)
	if err != nil {
		return nil, err
	}
	out := obj{}
	if includeDraftMarker {
		out["$schema"] = jsonSchemaDraft
	}
	for k, v := range root {
		out[k] = v
	}
	defs, err := r.renderDefs()
	if err != nil {
		return nil, err
	}
	if len(defs) > 0 {
		out["$defs"] = defs
	}
	return out, nil
}

// ToJSONSchemaBytes renders the document and marshals it.
func (r Ref) ToJSONSchemaBytes(includeDraftMarker bool) ([]byte, error) {
	doc, err := r.ToJSONSchema(includeDraftMarker)
	if err != nil {
		return nil, err
	}
	return json.Marshal(doc)
}

func (r Ref) renderDefs() (obj, error) {
	if len(r.graph.Defs) == 0 {
		return nil, nil
	}
	defs := obj{}
	for _, def := range r.graph.Defs {
		rendered, err := r.renderSchema(def.Body)
		if err != nil {
			return nil, err
		}
		// A definition's name is display-only, so it fills in a title the body
		// did not already provide rather than overriding one.
		if _, has := rendered["title"]; !has && def.Name != nil {
			rendered["title"] = *def.Name
		}
		defs[def.Id] = rendered
	}
	return defs, nil
}

// refPointer builds the $defs pointer for a definition, applying RFC 6901
// escaping so an id containing ~ or / still resolves.
func refPointer(id string) string {
	escaped := strings.ReplaceAll(id, "~", "~0")
	escaped = strings.ReplaceAll(escaped, "/", "~1")
	return "#/$defs/" + escaped
}

// integerSchema renders a narrow signed integer: the type's own range,
// narrowed by any signed bounds among its restrictions.
func integerSchema(min, max int64, rs *NumericRestrictions) obj {
	if rs != nil && rs.Min != nil && rs.Min.Kind == BoundSigned {
		min = maxInt64(min, rs.Min.Signed)
	}
	if rs != nil && rs.Max != nil && rs.Max.Kind == BoundSigned {
		max = minInt64(max, rs.Max.Signed)
	}
	return obj{"type": "integer", "minimum": min, "maximum": max}
}

// unsignedSchema renders a narrow unsigned integer the same way.
func unsignedSchema(max uint64, rs *NumericRestrictions) obj {
	var min uint64
	if rs != nil && rs.Min != nil && rs.Min.Kind == BoundUnsigned {
		min = rs.Min.Unsigned
	}
	if rs != nil && rs.Max != nil && rs.Max.Kind == BoundUnsigned && rs.Max.Unsigned < max {
		max = rs.Max.Unsigned
	}
	return obj{"type": "integer", "minimum": min, "maximum": max}
}

// floatSchema renders a float, with minimum and maximum only where the
// restrictions set float bounds.
func floatSchema(lo, hi float64, rs *NumericRestrictions) obj {
	out := obj{"type": "number"}
	if rs != nil && rs.Min != nil && rs.Min.Kind == BoundFloatBits {
		out["minimum"] = math.Max(math.Float64frombits(rs.Min.FloatBits), lo)
	}
	if rs != nil && rs.Max != nil && rs.Max.Kind == BoundFloatBits {
		out["maximum"] = math.Min(math.Float64frombits(rs.Max.FloatBits), hi)
	}
	return out
}

func maxInt64(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}

func minInt64(a, b int64) int64 {
	if a < b {
		return a
	}
	return b
}

// integerStringSchema is the shape wide integers take: a canonical base-10
// string, because a JSON number cannot carry the full 64-bit range.
func integerStringSchema(min, max string, signed bool, format string) obj {
	pattern := "^(?:0|[1-9][0-9]*)$"
	if signed {
		pattern = "^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$"
	}
	return obj{
		"type":            "string",
		"format":          format,
		"pattern":         pattern,
		"x-golem-minimum": min,
		"x-golem-maximum": max,
	}
}

func signed64Schema(rs *NumericRestrictions) obj {
	min, max := int64(math.MinInt64), int64(math.MaxInt64)
	if rs != nil && rs.Min != nil && rs.Min.Kind == BoundSigned {
		min = rs.Min.Signed
	}
	if rs != nil && rs.Max != nil && rs.Max.Kind == BoundSigned {
		max = rs.Max.Signed
	}
	return integerStringSchema(strconv.FormatInt(min, 10), strconv.FormatInt(max, 10), true, "int64")
}

func unsigned64Schema(rs *NumericRestrictions) obj {
	min, max := uint64(0), uint64(math.MaxUint64)
	if rs != nil && rs.Min != nil && rs.Min.Kind == BoundUnsigned {
		min = rs.Min.Unsigned
	}
	if rs != nil && rs.Max != nil && rs.Max.Kind == BoundUnsigned {
		max = rs.Max.Unsigned
	}
	return integerStringSchema(strconv.FormatUint(min, 10), strconv.FormatUint(max, 10), false, "uint64")
}

// base64URLLength is the encoded length of n raw bytes in base64url with no
// padding, used to turn a byte-count restriction into a string-length one.
func base64URLLength(n uint32) int64 {
	full := int64(n) / 3 * 4
	switch n % 3 {
	case 1:
		return full + 2
	case 2:
		return full + 3
	default:
		return full
	}
}

// renderSchema renders one node. A reference becomes a pointer rather than
// being followed, which is also what terminates a recursive type.
func (r Ref) renderSchema(t SchemaType) (obj, error) {
	if ref, isRef := t.Body.(RefType); isRef {
		if _, found := r.graph.Def(ref.Id); !found {
			return nil, fmt.Errorf("golem: unknown type %q", ref.Id)
		}
		return obj{"$ref": refPointer(ref.Id)}, nil
	}
	rendered, err := r.renderBody(t.Body)
	if err != nil {
		return nil, err
	}
	return attachMetadata(rendered, t.Metadata), nil
}

func (r Ref) renderBody(body SchemaTypeBody) (obj, error) {
	switch b := body.(type) {
	case BoolType:
		return obj{"type": "boolean"}, nil
	case S8Type:
		return integerSchema(math.MinInt8, math.MaxInt8, b.Restrictions), nil
	case S16Type:
		return integerSchema(math.MinInt16, math.MaxInt16, b.Restrictions), nil
	case S32Type:
		return integerSchema(math.MinInt32, math.MaxInt32, b.Restrictions), nil
	case S64Type:
		return signed64Schema(b.Restrictions), nil
	case U8Type:
		return unsignedSchema(math.MaxUint8, b.Restrictions), nil
	case U16Type:
		return unsignedSchema(math.MaxUint16, b.Restrictions), nil
	case U32Type:
		return unsignedSchema(math.MaxUint32, b.Restrictions), nil
	case U64Type:
		return unsigned64Schema(b.Restrictions), nil
	case F32Type:
		return floatSchema(-math.MaxFloat32, math.MaxFloat32, b.Restrictions), nil
	case F64Type:
		return floatSchema(-math.MaxFloat64, math.MaxFloat64, b.Restrictions), nil
	case CharType:
		return obj{"type": "string", "minLength": 1, "maxLength": 1}, nil
	case StringType:
		return obj{"type": "string"}, nil

	case RecordType:
		props := obj{}
		var required []string
		for _, f := range b.Fields {
			fs, err := r.renderSchema(f.Body)
			if err != nil {
				return nil, err
			}
			props[f.Name] = attachMetadata(fs, f.Metadata)
			// An option field may be omitted entirely, so it is not required;
			// an explicit null still satisfies the option's own schema.
			optional, err := r.resolvesToOption(f.Body)
			if err != nil {
				return nil, err
			}
			if !optional {
				required = append(required, f.Name)
			}
		}
		return obj{
			"type":                 "object",
			"properties":           props,
			"required":             requiredList(required),
			"additionalProperties": false,
		}, nil

	case VariantType:
		var oneOf []any
		for _, c := range b.Cases {
			if c.Payload == nil {
				oneOf = append(oneOf, obj{"const": c.Name})
				continue
			}
			payload, err := r.renderSchema(*c.Payload)
			if err != nil {
				return nil, err
			}
			oneOf = append(oneOf, obj{
				"type":                 "object",
				"properties":           obj{c.Name: payload},
				"required":             []string{c.Name},
				"additionalProperties": false,
			})
		}
		return obj{"oneOf": oneOf}, nil

	case EnumType:
		return obj{"type": "string", "enum": stringList(b.Cases)}, nil

	case FlagsType:
		return obj{
			"type":        "array",
			"items":       obj{"type": "string", "enum": stringList(b.Flags)},
			"uniqueItems": true,
		}, nil

	case TupleType:
		if len(b.Elements) == 0 {
			// prefixItems must be a non-empty array, so an empty tuple is
			// expressed purely by its length.
			return obj{"type": "array", "minItems": 0, "maxItems": 0}, nil
		}
		prefix := make([]any, 0, len(b.Elements))
		for _, e := range b.Elements {
			s, err := r.renderSchema(e)
			if err != nil {
				return nil, err
			}
			prefix = append(prefix, s)
		}
		return obj{
			"type":        "array",
			"prefixItems": prefix,
			"items":       false,
			"minItems":    len(b.Elements),
		}, nil

	case ListType:
		items, err := r.renderSchema(b.Element)
		if err != nil {
			return nil, err
		}
		return obj{"type": "array", "items": items}, nil

	case FixedListType:
		items, err := r.renderSchema(b.Element)
		if err != nil {
			return nil, err
		}
		return obj{"type": "array", "items": items, "minItems": b.Length, "maxItems": b.Length}, nil

	case MapType:
		key, err := r.renderSchema(b.Key)
		if err != nil {
			return nil, err
		}
		value, err := r.renderSchema(b.Value)
		if err != nil {
			return nil, err
		}
		// A map travels as an array of [key, value] pairs, so JSON object keys
		// do not have to be strings.
		pair := obj{
			"type":        "array",
			"prefixItems": []any{key, value},
			"items":       false,
			"minItems":    2,
			"maxItems":    2,
		}
		return obj{"type": "array", "items": pair}, nil

	case OptionType:
		inner, err := r.renderSchema(b.Inner)
		if err != nil {
			return nil, err
		}
		return obj{"oneOf": []any{obj{"type": "null"}, inner}}, nil

	case ResultType:
		okInner, err := r.renderSide(b.Ok)
		if err != nil {
			return nil, err
		}
		errInner, err := r.renderSide(b.Err)
		if err != nil {
			return nil, err
		}
		return obj{"oneOf": []any{
			obj{
				"type":                 "object",
				"properties":           obj{"ok": okInner},
				"required":             []string{"ok"},
				"additionalProperties": false,
			},
			obj{
				"type":                 "object",
				"properties":           obj{"err": errInner},
				"required":             []string{"err"},
				"additionalProperties": false,
			},
		}}, nil

	case TextType:
		return textSchema(b.Restrictions), nil
	case BinaryType:
		return binarySchema(b.Restrictions), nil
	case PathType:
		return pathSchema(b.Spec), nil
	case UrlType:
		return urlSchema(b.Restrictions), nil

	case DatetimeType:
		return obj{"type": "string", "format": "date-time"}, nil

	case DurationType:
		return obj{
			"type":                 "object",
			"properties":           obj{"nanoseconds": signed64Schema(nil)},
			"required":             []string{"nanoseconds"},
			"additionalProperties": false,
			"title":                "Duration in nanoseconds",
		}, nil

	case QuantityType:
		return quantitySchema(b.Spec), nil

	case UnionType:
		var oneOf []any
		for _, br := range b.Branches {
			branch, err := r.renderSchema(br.Body)
			if err != nil {
				return nil, err
			}
			oneOf = append(oneOf, applyDiscriminator(branch, br.Discriminator))
		}
		return obj{"oneOf": oneOf}, nil

	// Reflection packs and unpacks JSON, and none of these has a JSON form:
	// their schemas accept no value at all, as the host's reflection schemas
	// do.
	case SecretType:
		return hostManagedSchema("secret"), nil
	case QuotaTokenType:
		return hostManagedSchema("quota-token"), nil
	case PermissionCardType:
		return hostManagedSchema("permission-card"), nil

	case FutureType, StreamType:
		return obj{"not": obj{}}, nil
	}
	return nil, fmt.Errorf("golem: cannot render %T as JSON Schema", body)
}

func hostManagedSchema(kind string) obj {
	return obj{
		"not":         obj{},
		"description": "Host-managed " + kind + " capabilities cannot be supplied externally",
	}
}

// renderSide renders an optional result side; an absent side carries no value.
func (r Ref) renderSide(side *SchemaType) (obj, error) {
	if side == nil {
		return obj{"type": "null"}, nil
	}
	return r.renderSchema(*side)
}

// resolvesToOption reports whether a node is an option once references are
// followed, which decides whether a record field is required.
func (r Ref) resolvesToOption(t SchemaType) (bool, error) {
	body, err := r.At(t).body()
	if err != nil {
		return false, err
	}
	_, isOption := body.(OptionType)
	return isOption, nil
}

func textSchema(rs TextRestrictions) obj {
	text := obj{"type": "string"}
	if rs.MinLength != nil {
		text["minLength"] = *rs.MinLength
	}
	if rs.MaxLength != nil {
		text["maxLength"] = *rs.MaxLength
	}
	if rs.Regex != nil {
		text["pattern"] = *rs.Regex
	}
	language := obj{"type": "string"}
	if rs.Languages != nil {
		language["enum"] = *rs.Languages
	}
	return obj{
		"type":                 "object",
		"properties":           obj{"text": text, "language": language},
		"required":             []string{"text"},
		"additionalProperties": false,
	}
}

// mimeTypePattern is a bare type/subtype, without parameters — the form the
// canonical encoding accepts.
const mimeTypePattern = `^[A-Za-z0-9!#$&^_.+\-]+\/[A-Za-z0-9!#$&^_.+\-]+$`

// base64URLPattern matches canonical unpadded base64url: the unused low bits
// of a final partial group are zero.
const base64URLPattern = `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$`

func binarySchema(rs BinaryRestrictions) obj {
	// The restrictions count raw bytes, but the JSON field carries them
	// base64url-encoded, so the bounds are converted to encoded lengths.
	bytesField := obj{"type": "string", "contentEncoding": "base64url", "pattern": base64URLPattern}
	if rs.MinBytes != nil {
		bytesField["minLength"] = base64URLLength(*rs.MinBytes)
	}
	if rs.MaxBytes != nil {
		bytesField["maxLength"] = base64URLLength(*rs.MaxBytes)
	}
	mimeType := obj{"type": "string", "pattern": mimeTypePattern}
	if rs.MimeTypes != nil {
		mimeType["enum"] = *rs.MimeTypes
	}
	return obj{
		"type":                 "object",
		"properties":           obj{"bytes": bytesField, "mimeType": mimeType},
		"required":             []string{"bytes"},
		"additionalProperties": false,
	}
}

func pathSchema(spec PathSpec) obj {
	kind := map[PathKind]string{PathFile: "file", PathDirectory: "directory", PathAny: "any"}[spec.Kind]
	direction := map[PathDirection]string{
		PathInput: "input", PathOutput: "output", PathInOut: "inout",
	}[spec.Direction]
	out := obj{
		"type":   "string",
		"format": "file-path",
		"title":  direction + " " + kind + " path",
	}
	var description []string
	if spec.AllowedExtensions != nil {
		description = append(description, "Allowed extensions: "+strings.Join(*spec.AllowedExtensions, ", "))
	}
	if spec.AllowedMimeTypes != nil {
		description = append(description, "Allowed MIME types: "+strings.Join(*spec.AllowedMimeTypes, ", "))
	}
	if len(description) > 0 {
		out["description"] = strings.Join(description, "; ")
	}
	return out
}

func urlSchema(rs UrlRestrictions) obj {
	out := obj{"type": "string", "format": "uri", "title": "URL"}
	var description []string
	if rs.AllowedSchemes != nil {
		description = append(description, "Allowed schemes: "+strings.Join(*rs.AllowedSchemes, ", "))
	}
	if rs.AllowedHosts != nil {
		description = append(description, "Allowed hosts: "+strings.Join(*rs.AllowedHosts, ", "))
	}
	if len(description) > 0 {
		out["description"] = strings.Join(description, "; ")
	}
	return out
}

func quantitySchema(spec QuantitySpec) obj {
	out := obj{
		"type": "object",
		"properties": obj{
			"mantissa": signed64Schema(nil),
			"scale":    obj{"type": "integer"},
			"unit":     obj{"type": "string"},
		},
		"required":             []string{"mantissa", "scale", "unit"},
		"additionalProperties": false,
		"title":                "Quantity (" + spec.BaseUnit + ")",
	}
	var description []string
	if spec.Min != nil {
		description = append(description, "min: "+renderQuantity(*spec.Min))
	}
	if spec.Max != nil {
		description = append(description, "max: "+renderQuantity(*spec.Max))
	}
	if len(description) > 0 {
		out["description"] = strings.Join(description, "; ")
	}
	return out
}

func renderQuantity(q QuantityValue) string {
	return fmt.Sprintf("%de-%d %s", q.Mantissa, q.Scale, q.Unit)
}

// applyDiscriminator narrows a union branch's schema with the condition that
// selects it, so the oneOf is decidable by a reader.
func applyDiscriminator(schema obj, rule DiscriminatorRule) obj {
	var condition obj
	switch d := rule.(type) {
	case PrefixRule:
		condition = obj{"type": "string", "pattern": "^" + regexp.QuoteMeta(d.Value)}
	case SuffixRule:
		condition = obj{"type": "string", "pattern": regexp.QuoteMeta(d.Value) + "$"}
	case ContainsRule:
		condition = obj{"type": "string", "pattern": regexp.QuoteMeta(d.Value)}
	case RegexRule:
		condition = obj{"type": "string", "pattern": d.Pattern}
	case FieldEqualsRule:
		condition = obj{"type": "object", "required": []string{d.FieldName}}
		if d.Literal != nil {
			condition["properties"] = obj{d.FieldName: obj{"const": *d.Literal}}
		}
	case FieldAbsentRule:
		condition = obj{"type": "object", "not": obj{"required": []string{d.FieldName}}}
	default:
		return schema
	}
	return obj{"allOf": []any{schema, condition}}
}

// attachMetadata folds a node's documentation into the rendered schema without
// overwriting anything the body already said.
func attachMetadata(schema obj, md MetadataEnvelope) obj {
	if md.Doc != nil {
		if _, has := schema["description"]; !has {
			schema["description"] = *md.Doc
		}
	}
	if len(md.Examples) > 0 {
		if _, has := schema["examples"]; !has {
			examples := make([]any, 0, len(md.Examples))
			for _, e := range md.Examples {
				var decoded any
				if err := json.Unmarshal([]byte(e), &decoded); err != nil {
					// An example that is not canonical JSON is carried verbatim
					// rather than dropped.
					decoded = e
				}
				examples = append(examples, decoded)
			}
			schema["examples"] = examples
		}
	}
	if md.Deprecated != nil {
		schema["deprecated"] = true
	}
	return schema
}

// requiredList keeps required an array even when empty, which is what the host
// renderer emits.
func requiredList(names []string) []string {
	if names == nil {
		return []string{}
	}
	return names
}

func stringList(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}
