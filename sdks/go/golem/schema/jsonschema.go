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

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// JSON Schema rendering.
//
// The document follows the same guest-side shape the TypeScript SDK emits, and
// agrees with the host renderer on everything a caller can observe in a value:
// integers through 32 bits are JSON numbers with an explicit range, s64 and u64
// are canonical base-10 strings, a duration is an object with a `nanoseconds`
// string, and a quantity is `{mantissa, scale, unit}`. Union branches are
// inlined under `oneOf` rather than lifted into synthesised `$defs` entries;
// named definitions in the graph do become `$defs`.

// jsonSchemaDraft is the dialect every rendered document declares.
const jsonSchemaDraft = "https://json-schema.org/draft/2020-12/schema"

// obj is the rendered form of one schema node. Marshalling sorts the keys, so
// the output is byte-stable.
type obj = map[string]any

// ToJSONSchema renders the type as a JSON Schema document. includeDraftMarker
// adds the `$schema` dialect declaration, which belongs on a standalone
// document but is usually stripped when the schema is embedded (OpenAPI, for
// instance, rejects it).
func (r Ref) ToJSONSchema(includeDraftMarker bool) (any, error) {
	root, err := r.renderSchema(r.root)
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
	if len(r.graph.Defs) > 0 {
		defs := obj{}
		for _, def := range r.graph.Defs {
			rendered, err := r.renderSchema(def.Body)
			if err != nil {
				return nil, err
			}
			// A def's name is display-only, so it fills in a title the body did
			// not already provide rather than overriding one.
			if _, has := rendered["title"]; !has && def.Name.IsSome() {
				rendered["title"] = def.Name.Some()
			}
			defs[def.Id] = rendered
		}
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

// refPointer builds the `$defs` pointer for a named definition, applying RFC
// 6901 escaping to the member name.
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

// renderSchema renders one node. A ref is emitted as a pointer rather than
// followed, which is also what terminates recursive types.
func (r Ref) renderSchema(idx int32) (obj, error) {
	if idx < 0 || int(idx) >= len(r.graph.TypeNodes) {
		return nil, fmt.Errorf("schema: type node index %d out of range (%d nodes)", idx, len(r.graph.TypeNodes))
	}
	node := r.graph.TypeNodes[idx]
	body := node.Body
	if body.Tag() == types.SchemaTypeBodyRefType {
		defIdx := body.RefType()
		if defIdx < 0 || int(defIdx) >= len(r.graph.Defs) {
			return nil, fmt.Errorf("schema: ref target %d out of range (%d defs)", defIdx, len(r.graph.Defs))
		}
		return obj{"$ref": refPointer(r.graph.Defs[defIdx].Id)}, nil
	}
	rendered, err := r.renderBody(body)
	if err != nil {
		return nil, err
	}
	return attachMetadata(rendered, node.Metadata), nil
}

func (r Ref) renderBody(body types.SchemaTypeBody) (obj, error) {
	switch body.Tag() {
	case types.SchemaTypeBodyBoolType:
		return obj{"type": "boolean"}, nil
	case types.SchemaTypeBodyS8Type:
		return integerSchema(math.MinInt8, math.MaxInt8), nil
	case types.SchemaTypeBodyS16Type:
		return integerSchema(math.MinInt16, math.MaxInt16), nil
	case types.SchemaTypeBodyS32Type:
		return integerSchema(math.MinInt32, math.MaxInt32), nil
	case types.SchemaTypeBodyS64Type:
		return signed64Schema(), nil
	case types.SchemaTypeBodyU8Type:
		return integerSchema(0, math.MaxUint8), nil
	case types.SchemaTypeBodyU16Type:
		return integerSchema(0, math.MaxUint16), nil
	case types.SchemaTypeBodyU32Type:
		return integerSchema(0, math.MaxUint32), nil
	case types.SchemaTypeBodyU64Type:
		return unsigned64Schema(), nil
	case types.SchemaTypeBodyF32Type, types.SchemaTypeBodyF64Type:
		return obj{"type": "number"}, nil
	case types.SchemaTypeBodyCharType:
		return obj{"type": "string", "minLength": 1, "maxLength": 1}, nil
	case types.SchemaTypeBodyStringType:
		return obj{"type": "string"}, nil

	case types.SchemaTypeBodyRecordType:
		props := obj{}
		var required []string
		for _, f := range body.RecordType() {
			fs, err := r.renderSchema(f.Body)
			if err != nil {
				return nil, err
			}
			props[f.Name] = attachMetadata(fs, f.Metadata)
			// An option field may be omitted entirely, so it is not required;
			// an explicit null still satisfies the option's own schema.
			opt, err := r.resolvesToOption(f.Body)
			if err != nil {
				return nil, err
			}
			if !opt {
				required = append(required, f.Name)
			}
		}
		return obj{
			"type":                 "object",
			"properties":           props,
			"required":             requiredList(required),
			"additionalProperties": false,
		}, nil

	case types.SchemaTypeBodyVariantType:
		var oneOf []any
		for _, c := range body.VariantType() {
			if c.Payload.IsNone() {
				oneOf = append(oneOf, obj{"const": c.Name})
				continue
			}
			payload, err := r.renderSchema(c.Payload.Some())
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

	case types.SchemaTypeBodyEnumType:
		return obj{"type": "string", "enum": stringList(body.EnumType())}, nil

	case types.SchemaTypeBodyFlagsType:
		return obj{
			"type":        "array",
			"items":       obj{"type": "string", "enum": stringList(body.FlagsType())},
			"uniqueItems": true,
		}, nil

	case types.SchemaTypeBodyTupleType:
		elems := body.TupleType()
		if len(elems) == 0 {
			// prefixItems must be a non-empty array, so an empty tuple is
			// expressed purely by its length.
			return obj{"type": "array", "minItems": 0, "maxItems": 0}, nil
		}
		prefix := make([]any, len(elems))
		for i, e := range elems {
			s, err := r.renderSchema(e)
			if err != nil {
				return nil, err
			}
			prefix[i] = s
		}
		return obj{
			"type":        "array",
			"prefixItems": prefix,
			"items":       false,
			"minItems":    len(elems),
		}, nil

	case types.SchemaTypeBodyListType:
		items, err := r.renderSchema(body.ListType())
		if err != nil {
			return nil, err
		}
		return obj{"type": "array", "items": items}, nil

	case types.SchemaTypeBodyFixedListType:
		spec := body.FixedListType()
		items, err := r.renderSchema(spec.Element)
		if err != nil {
			return nil, err
		}
		return obj{
			"type":     "array",
			"items":    items,
			"minItems": spec.Length,
			"maxItems": spec.Length,
		}, nil

	case types.SchemaTypeBodyMapType:
		spec := body.MapType()
		key, err := r.renderSchema(spec.Key)
		if err != nil {
			return nil, err
		}
		value, err := r.renderSchema(spec.Value)
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

	case types.SchemaTypeBodyOptionType:
		inner, err := r.renderSchema(body.OptionType())
		if err != nil {
			return nil, err
		}
		return obj{"oneOf": []any{obj{"type": "null"}, inner}}, nil

	case types.SchemaTypeBodyResultType:
		spec := body.ResultType()
		okInner, err := r.renderSide(spec.Ok)
		if err != nil {
			return nil, err
		}
		errInner, err := r.renderSide(spec.Err)
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

	case types.SchemaTypeBodyTextType:
		return textSchema(body.TextType()), nil
	case types.SchemaTypeBodyBinaryType:
		return binarySchema(body.BinaryType()), nil
	case types.SchemaTypeBodyPathType:
		return pathSchema(body.PathType()), nil
	case types.SchemaTypeBodyUrlType:
		return urlSchema(body.UrlType()), nil

	case types.SchemaTypeBodyDatetimeType:
		return obj{"type": "string", "format": "date-time"}, nil

	case types.SchemaTypeBodyDurationType:
		return obj{
			"type":                 "object",
			"properties":           obj{"nanoseconds": signed64Schema()},
			"required":             []string{"nanoseconds"},
			"additionalProperties": false,
			"title":                "Duration in nanoseconds",
		}, nil

	case types.SchemaTypeBodyQuantityType:
		return quantitySchema(body.QuantityType()), nil

	case types.SchemaTypeBodyUnionType:
		var oneOf []any
		for _, b := range body.UnionType().Branches {
			branch, err := r.renderSchema(b.Body)
			if err != nil {
				return nil, err
			}
			oneOf = append(oneOf, applyDiscriminator(branch, b.Discriminator))
		}
		return obj{"oneOf": oneOf}, nil

	case types.SchemaTypeBodySecretType:
		return obj{"writeOnly": true, "x-golem-capability": "secret"}, nil
	case types.SchemaTypeBodyQuotaTokenType:
		return obj{"writeOnly": true, "x-golem-capability": "quota-token"}, nil
	case types.SchemaTypeBodyPermissionCardType:
		return obj{"writeOnly": true, "x-golem-capability": "permission-card"}, nil

	case types.SchemaTypeBodyFutureType, types.SchemaTypeBodyStreamType:
		return obj{"type": "null", "description": "WASI P3 placeholder"}, nil
	}
	return nil, fmt.Errorf("schema: cannot render type node (tag %d) as JSON Schema", body.Tag())
}

// renderSide renders an optional result side; an absent side carries no value.
func (r Ref) renderSide(side witTypes.Option[int32]) (obj, error) {
	if side.IsNone() {
		return obj{"type": "null"}, nil
	}
	return r.renderSchema(side.Some())
}

func textSchema(rs types.TextRestrictions) obj {
	text := obj{"type": "string"}
	if rs.MinLength.IsSome() {
		text["minLength"] = rs.MinLength.Some()
	}
	if rs.MaxLength.IsSome() {
		text["maxLength"] = rs.MaxLength.Some()
	}
	if rs.Regex.IsSome() {
		text["pattern"] = rs.Regex.Some()
	}
	out := obj{
		"type":                 "object",
		"properties":           obj{"text": text, "language": obj{"type": "string"}},
		"required":             []string{"text"},
		"additionalProperties": false,
	}
	if rs.Languages.IsSome() {
		out["description"] = "Allowed languages: " + strings.Join(rs.Languages.Some(), ", ")
	}
	return out
}

// mimeTypePattern constrains a MIME type to `type/subtype` with optional
// parameters.
const mimeTypePattern = `^[A-Za-z0-9!#$%&'*+.^_` + "`" + `|~-]+/[A-Za-z0-9!#$%&'*+.^_` + "`" + `|~-]+(?:;.*)?$`

func binarySchema(rs types.BinaryRestrictions) obj {
	// The restrictions count raw bytes, but the JSON field carries them
	// base64url-encoded, so the bounds are converted to encoded lengths.
	bytes := obj{"type": "string", "contentEncoding": "base64url"}
	if rs.MinBytes.IsSome() {
		bytes["minLength"] = base64URLLength(rs.MinBytes.Some())
	}
	if rs.MaxBytes.IsSome() {
		bytes["maxLength"] = base64URLLength(rs.MaxBytes.Some())
	}
	out := obj{
		"type": "object",
		"properties": obj{
			"bytes":    bytes,
			"mimeType": obj{"type": "string", "pattern": mimeTypePattern},
		},
		"required":             []string{"bytes"},
		"additionalProperties": false,
	}
	if rs.MimeTypes.IsSome() {
		out["description"] = "Allowed MIME types: " + strings.Join(rs.MimeTypes.Some(), ", ")
	}
	return out
}

func pathSchema(spec types.PathSpec) obj {
	kind := map[uint8]string{
		types.PathKindFile:      "file",
		types.PathKindDirectory: "directory",
		types.PathKindAny:       "any",
	}[spec.Kind]
	direction := map[uint8]string{
		types.PathDirectionInput:  "input",
		types.PathDirectionOutput: "output",
		types.PathDirectionInOut:  "inout",
	}[spec.Direction]
	out := obj{
		"type":   "string",
		"format": "file-path",
		"title":  direction + " " + kind + " path",
	}
	var description []string
	if spec.AllowedExtensions.IsSome() {
		description = append(description, "Allowed extensions: "+strings.Join(spec.AllowedExtensions.Some(), ", "))
	}
	if spec.AllowedMimeTypes.IsSome() {
		description = append(description, "Allowed MIME types: "+strings.Join(spec.AllowedMimeTypes.Some(), ", "))
	}
	if len(description) > 0 {
		out["description"] = strings.Join(description, "; ")
	}
	return out
}

func urlSchema(rs types.UrlRestrictions) obj {
	out := obj{"type": "string", "format": "uri", "title": "URL"}
	var description []string
	if rs.AllowedSchemes.IsSome() {
		description = append(description, "Allowed schemes: "+strings.Join(rs.AllowedSchemes.Some(), ", "))
	}
	if rs.AllowedHosts.IsSome() {
		description = append(description, "Allowed hosts: "+strings.Join(rs.AllowedHosts.Some(), ", "))
	}
	if len(description) > 0 {
		out["description"] = strings.Join(description, "; ")
	}
	return out
}

func quantitySchema(spec types.QuantitySpec) obj {
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
	if spec.Min.IsSome() {
		description = append(description, "min: "+renderQuantity(spec.Min.Some()))
	}
	if spec.Max.IsSome() {
		description = append(description, "max: "+renderQuantity(spec.Max.Some()))
	}
	if len(description) > 0 {
		out["description"] = strings.Join(description, "; ")
	}
	return out
}

func renderQuantity(q types.QuantityValue) string {
	return fmt.Sprintf("%de-%d %s", q.Mantissa, q.Scale, q.Unit)
}

// applyDiscriminator narrows a union branch's schema with the condition that
// selects it, so the `oneOf` is decidable by a reader.
func applyDiscriminator(schema obj, rule types.DiscriminatorRule) obj {
	var condition obj
	switch rule.Tag() {
	case types.DiscriminatorRulePrefix:
		condition = obj{"type": "string", "pattern": "^" + regexp.QuoteMeta(rule.Prefix())}
	case types.DiscriminatorRuleSuffix:
		condition = obj{"type": "string", "pattern": regexp.QuoteMeta(rule.Suffix()) + "$"}
	case types.DiscriminatorRuleContains:
		condition = obj{"type": "string", "pattern": regexp.QuoteMeta(rule.Contains())}
	case types.DiscriminatorRuleRegex:
		condition = obj{"type": "string", "pattern": rule.Regex()}
	case types.DiscriminatorRuleFieldEquals:
		d := rule.FieldEquals()
		condition = obj{"type": "object", "required": []string{d.FieldName}}
		if d.Literal.IsSome() {
			condition["properties"] = obj{d.FieldName: obj{"const": d.Literal.Some()}}
		}
	case types.DiscriminatorRuleFieldAbsent:
		condition = obj{"type": "object", "not": obj{"required": []string{rule.FieldAbsent()}}}
	default:
		return schema
	}
	return obj{"allOf": []any{schema, condition}}
}

// attachMetadata folds a node's documentation into the rendered schema without
// overwriting anything the body already said.
func attachMetadata(schema obj, md types.MetadataEnvelope) obj {
	if md.Doc.IsSome() {
		if _, has := schema["description"]; !has {
			schema["description"] = md.Doc.Some()
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
	if md.Deprecated.IsSome() {
		schema["deprecated"] = true
	}
	return schema
}

// requiredList keeps `required` an array even when empty, which is what the
// host renderer emits.
func requiredList(names []string) []string {
	if names == nil {
		return []string{}
	}
	return names
}

// resolvesToOption reports whether a node is an option once ref indirection is
// followed, which decides whether a record field is required.
func (r Ref) resolvesToOption(idx int32) (bool, error) {
	body, _, err := r.node(idx)
	if err != nil {
		return false, err
	}
	return body.Tag() == types.SchemaTypeBodyOptionType, nil
}

func stringList(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}
