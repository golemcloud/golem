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

func integerSchema(min, max int64) obj {
	return obj{"type": "integer", "minimum": min, "maximum": max}
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

func signed64Schema() obj {
	return integerStringSchema("-9223372036854775808", "9223372036854775807", true, "int64")
}

func unsigned64Schema() obj {
	return integerStringSchema("0", "18446744073709551615", false, "uint64")
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
		return integerSchema(math.MinInt8, math.MaxInt8), nil
	case S16Type:
		return integerSchema(math.MinInt16, math.MaxInt16), nil
	case S32Type:
		return integerSchema(math.MinInt32, math.MaxInt32), nil
	case S64Type:
		return signed64Schema(), nil
	case U8Type:
		return integerSchema(0, math.MaxUint8), nil
	case U16Type:
		return integerSchema(0, math.MaxUint16), nil
	case U32Type:
		return integerSchema(0, math.MaxUint32), nil
	case U64Type:
		return unsigned64Schema(), nil
	case F32Type, F64Type:
		return obj{"type": "number"}, nil
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
			"properties":           obj{"nanoseconds": signed64Schema()},
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

	case SecretType:
		return obj{"writeOnly": true, "x-golem-capability": "secret"}, nil
	case QuotaTokenType:
		return obj{"writeOnly": true, "x-golem-capability": "quota-token"}, nil
	case PermissionCardType:
		return obj{"writeOnly": true, "x-golem-capability": "permission-card"}, nil

	case FutureType, StreamType:
		return obj{"type": "null", "description": "WASI P3 placeholder"}, nil
	}
	return nil, fmt.Errorf("golem: cannot render %T as JSON Schema", body)
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
	out := obj{
		"type":                 "object",
		"properties":           obj{"text": text, "language": obj{"type": "string"}},
		"required":             []string{"text"},
		"additionalProperties": false,
	}
	if rs.Languages != nil {
		out["description"] = "Allowed languages: " + strings.Join(*rs.Languages, ", ")
	}
	return out
}

// mimeTypePattern constrains a MIME type to type/subtype with optional
// parameters.
const mimeTypePattern = `^[A-Za-z0-9!#$%&'*+.^_` + "`" + `|~-]+/[A-Za-z0-9!#$%&'*+.^_` + "`" + `|~-]+(?:;.*)?$`

func binarySchema(rs BinaryRestrictions) obj {
	// The restrictions count raw bytes, but the JSON field carries them
	// base64url-encoded, so the bounds are converted to encoded lengths.
	bytesField := obj{"type": "string", "contentEncoding": "base64url"}
	if rs.MinBytes != nil {
		bytesField["minLength"] = base64URLLength(*rs.MinBytes)
	}
	if rs.MaxBytes != nil {
		bytesField["maxLength"] = base64URLLength(*rs.MaxBytes)
	}
	out := obj{
		"type": "object",
		"properties": obj{
			"bytes":    bytesField,
			"mimeType": obj{"type": "string", "pattern": mimeTypePattern},
		},
		"required":             []string{"bytes"},
		"additionalProperties": false,
	}
	if rs.MimeTypes != nil {
		out["description"] = "Allowed MIME types: " + strings.Join(*rs.MimeTypes, ", ")
	}
	return out
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
			"mantissa": signed64Schema(),
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
