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
)

// Reading a type graph in the schema-native wire form. See wirejson.go for what
// that form is and how it differs from canonical JSON.
//
// A graph arrives whole: a generated client embeds the schema it was generated
// against, and a caller that discovers an agent is handed one. Nothing here
// produces a graph, because nothing outside the server invents one.

type wireGraph struct {
	Defs []wireTypeDef   `json:"defs"`
	Root json.RawMessage `json:"root"`
}

type wireTypeDef struct {
	Id   string          `json:"id"`
	Name *string         `json:"name"`
	Body json.RawMessage `json:"body"`
}

func (g wireGraph) toModel() (SchemaGraph, error) {
	out := SchemaGraph{Defs: make([]SchemaTypeDef, 0, len(g.Defs))}
	for _, def := range g.Defs {
		body, err := wireToType(def.Body)
		if err != nil {
			return SchemaGraph{}, fmt.Errorf("golem: type %q: %w", def.Id, err)
		}
		out.Defs = append(out.Defs, SchemaTypeDef{Id: def.Id, Name: def.Name, Body: body})
	}
	root, err := wireToType(g.Root)
	if err != nil {
		return SchemaGraph{}, fmt.Errorf("golem: graph root: %w", err)
	}
	out.Root = root
	return out, nil
}

func wireToType(data json.RawMessage) (SchemaType, error) {
	if len(data) == 0 {
		return SchemaType{}, fmt.Errorf("golem: missing type")
	}
	var node wireNode
	if err := json.Unmarshal(data, &node); err != nil {
		return SchemaType{}, fmt.Errorf("golem: malformed type: %w", err)
	}
	body, metadata, err := wireToTypeBody(node)
	if err != nil {
		return SchemaType{}, err
	}
	return SchemaType{Body: body, Metadata: metadata}, nil
}

func wireToTypeBody(node wireNode) (SchemaTypeBody, MetadataEnvelope, error) {
	switch node.Kind {
	case "ref":
		return readType(node, func(p wireRefType) SchemaTypeBody { return RefType{Id: p.Id} })
	case "bool":
		return readType(node, func(wireBareType) SchemaTypeBody { return BoolType{} })
	case "char":
		return readType(node, func(wireBareType) SchemaTypeBody { return CharType{} })
	case "string":
		return readType(node, func(wireBareType) SchemaTypeBody { return StringType{} })
	case "datetime":
		return readType(node, func(wireBareType) SchemaTypeBody { return DatetimeType{} })
	case "duration":
		return readType(node, func(wireBareType) SchemaTypeBody { return DurationType{} })

	case "s8":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return S8Type{Restrictions: p.model()} })
	case "s16":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return S16Type{Restrictions: p.model()} })
	case "s32":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return S32Type{Restrictions: p.model()} })
	case "s64":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return S64Type{Restrictions: p.model()} })
	case "u8":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return U8Type{Restrictions: p.model()} })
	case "u16":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return U16Type{Restrictions: p.model()} })
	case "u32":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return U32Type{Restrictions: p.model()} })
	case "u64":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return U64Type{Restrictions: p.model()} })
	case "f32":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return F32Type{Restrictions: p.model()} })
	case "f64":
		return readType(node, func(p wireNumericType) SchemaTypeBody { return F64Type{Restrictions: p.model()} })

	case "record":
		return readTypeErr(node, func(p wireRecordType) (SchemaTypeBody, error) {
			fields := make([]NamedField, 0, len(p.Fields))
			for _, f := range p.Fields {
				body, err := wireToType(f.Body)
				if err != nil {
					return nil, fmt.Errorf("golem: record field %q: %w", f.Name, err)
				}
				fields = append(fields, NamedField{
					Name: f.Name, Body: body, Metadata: f.Metadata.model(),
				})
			}
			return RecordType{Fields: fields}, nil
		})
	case "variant":
		return readTypeErr(node, func(p wireVariantType) (SchemaTypeBody, error) {
			cases := make([]VariantCase, 0, len(p.Cases))
			for _, c := range p.Cases {
				payload, err := wireToOptionalType(c.Payload)
				if err != nil {
					return nil, fmt.Errorf("golem: variant case %q: %w", c.Name, err)
				}
				cases = append(cases, VariantCase{
					Name: c.Name, Payload: payload, Metadata: c.Metadata.model(),
				})
			}
			return VariantType{Cases: cases}, nil
		})
	case "enum":
		return readType(node, func(p wireEnumType) SchemaTypeBody { return EnumType{Cases: p.Cases} })
	case "flags":
		return readType(node, func(p wireFlagsType) SchemaTypeBody { return FlagsType{Flags: p.Flags} })
	case "tuple":
		return readTypeErr(node, func(p wireTupleType) (SchemaTypeBody, error) {
			elements := make([]SchemaType, 0, len(p.Elements))
			for i, raw := range p.Elements {
				element, err := wireToType(raw)
				if err != nil {
					return nil, fmt.Errorf("golem: tuple element %d: %w", i, err)
				}
				elements = append(elements, element)
			}
			return TupleType{Elements: elements}, nil
		})
	case "list":
		return readTypeErr(node, func(p wireListType) (SchemaTypeBody, error) {
			element, err := wireToType(p.Element)
			return ListType{Element: element}, err
		})
	case "fixed-list":
		return readTypeErr(node, func(p wireFixedListType) (SchemaTypeBody, error) {
			element, err := wireToType(p.Element)
			return FixedListType{Element: element, Length: p.Length}, err
		})
	case "map":
		return readTypeErr(node, func(p wireMapType) (SchemaTypeBody, error) {
			key, err := wireToType(p.Key)
			if err != nil {
				return nil, fmt.Errorf("golem: map key: %w", err)
			}
			value, err := wireToType(p.Value)
			if err != nil {
				return nil, fmt.Errorf("golem: map value: %w", err)
			}
			return MapType{Key: key, Value: value}, nil
		})
	case "option":
		return readTypeErr(node, func(p wireOptionType) (SchemaTypeBody, error) {
			inner, err := wireToType(p.Inner)
			return OptionType{Inner: inner}, err
		})
	case "result":
		return readTypeErr(node, func(p wireResultType) (SchemaTypeBody, error) {
			ok, err := wireToOptionalType(p.Spec.Ok)
			if err != nil {
				return nil, fmt.Errorf("golem: result ok: %w", err)
			}
			bad, err := wireToOptionalType(p.Spec.Err)
			if err != nil {
				return nil, fmt.Errorf("golem: result err: %w", err)
			}
			return ResultType{Ok: ok, Err: bad}, nil
		})

	case "text":
		return readType(node, func(p wireTextType) SchemaTypeBody {
			return TextType{Restrictions: TextRestrictions(p.Restrictions)}
		})
	case "binary":
		return readType(node, func(p wireBinaryType) SchemaTypeBody {
			return BinaryType{Restrictions: BinaryRestrictions(p.Restrictions)}
		})
	case "url":
		return readType(node, func(p wireUrlType) SchemaTypeBody {
			return UrlType{Restrictions: UrlRestrictions(p.Restrictions)}
		})
	case "path":
		return readTypeErr(node, func(p wirePathType) (SchemaTypeBody, error) {
			spec, err := p.Spec.model()
			return PathType{Spec: spec}, err
		})
	case "quantity":
		return readType(node, func(p wireQuantityType) SchemaTypeBody {
			return QuantityType{Spec: p.Spec.model()}
		})
	case "union":
		return readTypeErr(node, func(p wireUnionType) (SchemaTypeBody, error) {
			branches := make([]UnionBranch, 0, len(p.Spec.Branches))
			for _, b := range p.Spec.Branches {
				body, err := wireToType(b.Body)
				if err != nil {
					return nil, fmt.Errorf("golem: union branch %q: %w", b.Tag, err)
				}
				rule, err := b.Discriminator.model()
				if err != nil {
					return nil, fmt.Errorf("golem: union branch %q: %w", b.Tag, err)
				}
				branches = append(branches, UnionBranch{
					Tag: b.Tag, Body: body, Discriminator: rule, Metadata: b.Metadata.model(),
				})
			}
			return UnionType{Branches: branches}, nil
		})

	case "secret":
		return readTypeErr(node, func(p wireSecretType) (SchemaTypeBody, error) {
			// An absent inner type means a secret string, which is what the
			// server's serde default supplies.
			if len(p.Spec.Inner) == 0 {
				return SecretType{
					Inner:    SchemaType{Body: StringType{}},
					Category: p.Spec.Category,
				}, nil
			}
			inner, err := wireToType(p.Spec.Inner)
			return SecretType{Inner: inner, Category: p.Spec.Category}, err
		})
	case "quota-token":
		return readType(node, func(p wireQuotaTokenType) SchemaTypeBody {
			return QuotaTokenType{ResourceName: p.Spec.ResourceName}
		})
	case "permission-card":
		return readType(node, func(p wirePermissionCardType) SchemaTypeBody {
			return PermissionCardType{Polymorphic: p.Spec.Polymorphic}
		})

	case "future":
		return readTypeErr(node, func(p wireInnerType) (SchemaTypeBody, error) {
			inner, err := wireToOptionalType(p.Inner)
			return FutureType{Item: inner}, err
		})
	case "stream":
		return readTypeErr(node, func(p wireInnerType) (SchemaTypeBody, error) {
			inner, err := wireToOptionalType(p.Inner)
			return StreamType{Item: inner}, err
		})
	}
	return nil, MetadataEnvelope{}, fmt.Errorf("golem: unsupported schema type kind %q", node.Kind)
}

// readType decodes a type node's payload and builds its body; readTypeErr does
// the same for a case that can still reject what it decoded. Both pull the
// metadata envelope out of the same payload, since every case carries one.
func readType[T wireMetadataCarrier](
	node wireNode, build func(T) SchemaTypeBody,
) (SchemaTypeBody, MetadataEnvelope, error) {
	return readTypeErr(node, func(p T) (SchemaTypeBody, error) { return build(p), nil })
}

func readTypeErr[T wireMetadataCarrier](
	node wireNode, build func(T) (SchemaTypeBody, error),
) (SchemaTypeBody, MetadataEnvelope, error) {
	var payload T
	if len(node.Value) > 0 {
		if err := json.Unmarshal(node.Value, &payload); err != nil {
			return nil, MetadataEnvelope{}, fmt.Errorf("golem: %s type: %w", node.Kind, err)
		}
	}
	body, err := build(payload)
	if err != nil {
		return nil, MetadataEnvelope{}, err
	}
	return body, payload.metadata().model(), nil
}

func wireToOptionalType(data json.RawMessage) (*SchemaType, error) {
	if len(data) == 0 || string(data) == "null" {
		return nil, nil
	}
	t, err := wireToType(data)
	if err != nil {
		return nil, err
	}
	return &t, nil
}

// --- Payload shapes ------------------------------------------------------

// wireMetadataCarrier is every type payload: each one carries a metadata
// envelope, which is what lets one helper read the envelope for all of them.
type wireMetadataCarrier interface {
	metadata() wireMetadata
}

type wireMetadata struct {
	Doc        *string   `json:"doc"`
	Aliases    []string  `json:"aliases"`
	Examples   []string  `json:"examples"`
	Deprecated *string   `json:"deprecated"`
	Role       *wireRole `json:"role"`
}

func (m wireMetadata) model() MetadataEnvelope {
	out := MetadataEnvelope{
		Doc:        m.Doc,
		Aliases:    m.Aliases,
		Examples:   m.Examples,
		Deprecated: m.Deprecated,
	}
	if m.Role != nil {
		role := m.Role.model()
		out.Role = &role
	}
	return out
}

// wireRole is an open registry: a tag this build does not know is kept as its
// own name rather than dropped, so the producer's intent survives.
type wireRole struct {
	Tag   string `json:"tag"`
	Value string `json:"value"`
}

func (r wireRole) model() Role {
	if r.Tag == "other" {
		return Role(r.Value)
	}
	return Role(r.Tag)
}

// wireBase is embedded by every type payload, supplying the metadata envelope.
type wireBase struct {
	Metadata wireMetadata `json:"metadata"`
}

func (b wireBase) metadata() wireMetadata { return b.Metadata }

type wireBareType struct{ wireBase }

type wireRefType struct {
	wireBase
	Id string `json:"id"`
}

type wireNumericType struct {
	wireBase
	Restrictions *wireNumericRestrictions `json:"restrictions"`
}

func (n wireNumericType) model() *NumericRestrictions {
	if n.Restrictions == nil {
		return nil
	}
	return &NumericRestrictions{
		Min:  n.Restrictions.Min.model(),
		Max:  n.Restrictions.Max.model(),
		Unit: n.Restrictions.Unit,
	}
}

type wireNumericRestrictions struct {
	Min  *wireNumericBound `json:"min"`
	Max  *wireNumericBound `json:"max"`
	Unit *string           `json:"unit"`
}

// wireNumericBound keeps a bound in whichever width can hold it, which is why
// the wire form tags it rather than sending a bare number.
type wireNumericBound struct {
	Kind  string          `json:"kind"`
	Value json.RawMessage `json:"value"`
}

func (b *wireNumericBound) model() *NumericBound {
	if b == nil {
		return nil
	}
	out := &NumericBound{}
	switch b.Kind {
	case "unsigned":
		out.Kind = BoundUnsigned
		_ = json.Unmarshal(b.Value, &out.Unsigned)
	case "float-bits":
		out.Kind = BoundFloatBits
		_ = json.Unmarshal(b.Value, &out.FloatBits)
	default:
		out.Kind = BoundSigned
		_ = json.Unmarshal(b.Value, &out.Signed)
	}
	return out
}

type wireNamedField struct {
	Name     string          `json:"name"`
	Body     json.RawMessage `json:"body"`
	Metadata wireMetadata    `json:"metadata"`
}

type wireRecordType struct {
	wireBase
	Fields []wireNamedField `json:"fields"`
}

type wireVariantCase struct {
	Name     string          `json:"name"`
	Payload  json.RawMessage `json:"payload"`
	Metadata wireMetadata    `json:"metadata"`
}

type wireVariantType struct {
	wireBase
	Cases []wireVariantCase `json:"cases"`
}

type wireEnumType struct {
	wireBase
	Cases []string `json:"cases"`
}

type wireFlagsType struct {
	wireBase
	Flags []string `json:"flags"`
}

type wireTupleType struct {
	wireBase
	Elements []json.RawMessage `json:"elements"`
}

type wireListType struct {
	wireBase
	Element json.RawMessage `json:"element"`
}

type wireFixedListType struct {
	wireBase
	Element json.RawMessage `json:"element"`
	Length  uint32          `json:"length"`
}

type wireMapType struct {
	wireBase
	Key   json.RawMessage `json:"key"`
	Value json.RawMessage `json:"value"`
}

type wireOptionType struct {
	wireBase
	Inner json.RawMessage `json:"inner"`
}

type wireInnerType struct {
	wireBase
	Inner json.RawMessage `json:"inner"`
}

type wireResultType struct {
	wireBase
	Spec struct {
		Ok  json.RawMessage `json:"ok"`
		Err json.RawMessage `json:"err"`
	} `json:"spec"`
}

type wireTextRestrictions struct {
	Languages *[]string `json:"languages"`
	MinLength *uint32   `json:"minLength"`
	MaxLength *uint32   `json:"maxLength"`
	Regex     *string   `json:"regex"`
}

type wireTextType struct {
	wireBase
	Restrictions wireTextRestrictions `json:"restrictions"`
}

type wireBinaryRestrictions struct {
	MimeTypes *[]string `json:"mimeTypes"`
	MinBytes  *uint32   `json:"minBytes"`
	MaxBytes  *uint32   `json:"maxBytes"`
}

type wireBinaryType struct {
	wireBase
	Restrictions wireBinaryRestrictions `json:"restrictions"`
}

type wireUrlRestrictions struct {
	AllowedSchemes *[]string `json:"allowedSchemes"`
	AllowedHosts   *[]string `json:"allowedHosts"`
}

type wireUrlType struct {
	wireBase
	Restrictions wireUrlRestrictions `json:"restrictions"`
}

type wirePathSpec struct {
	Direction         string    `json:"direction"`
	Kind              string    `json:"kind"`
	AllowedMimeTypes  *[]string `json:"allowedMimeTypes"`
	AllowedExtensions *[]string `json:"allowedExtensions"`
}

func (p wirePathSpec) model() (PathSpec, error) {
	out := PathSpec{
		AllowedMimeTypes:  p.AllowedMimeTypes,
		AllowedExtensions: p.AllowedExtensions,
	}
	switch p.Direction {
	case "input":
		out.Direction = PathInput
	case "output":
		out.Direction = PathOutput
	case "in-out":
		out.Direction = PathInOut
	default:
		return PathSpec{}, fmt.Errorf("golem: unknown path direction %q", p.Direction)
	}
	switch p.Kind {
	case "file":
		out.Kind = PathFile
	case "directory":
		out.Kind = PathDirectory
	case "any":
		out.Kind = PathAny
	default:
		return PathSpec{}, fmt.Errorf("golem: unknown path kind %q", p.Kind)
	}
	return out, nil
}

type wirePathType struct {
	wireBase
	Spec wirePathSpec `json:"spec"`
}

type wireQuantitySpec struct {
	BaseUnit        string             `json:"baseUnit"`
	AllowedSuffixes []string           `json:"allowedSuffixes"`
	Min             *wireQuantityValue `json:"min"`
	Max             *wireQuantityValue `json:"max"`
}

func (q wireQuantitySpec) model() QuantitySpec {
	out := QuantitySpec{BaseUnit: q.BaseUnit, AllowedSuffixes: q.AllowedSuffixes}
	if q.Min != nil {
		min := QuantityValue(*q.Min)
		out.Min = &min
	}
	if q.Max != nil {
		max := QuantityValue(*q.Max)
		out.Max = &max
	}
	return out
}

type wireQuantityType struct {
	wireBase
	Spec wireQuantitySpec `json:"spec"`
}

type wireDiscriminator struct {
	Rule  string          `json:"rule"`
	Value json.RawMessage `json:"value"`
}

func (d wireDiscriminator) model() (DiscriminatorRule, error) {
	read := func(dst any) error {
		if len(d.Value) == 0 {
			return fmt.Errorf("golem: discriminator %q carries no value", d.Rule)
		}
		return json.Unmarshal(d.Value, dst)
	}
	switch d.Rule {
	case "prefix":
		var p struct {
			Prefix string `json:"prefix"`
		}
		if err := read(&p); err != nil {
			return nil, err
		}
		return PrefixRule{Value: p.Prefix}, nil
	case "suffix":
		var p struct {
			Suffix string `json:"suffix"`
		}
		if err := read(&p); err != nil {
			return nil, err
		}
		return SuffixRule{Value: p.Suffix}, nil
	case "contains":
		var p struct {
			Substring string `json:"substring"`
		}
		if err := read(&p); err != nil {
			return nil, err
		}
		return ContainsRule{Value: p.Substring}, nil
	case "regex":
		var p struct {
			Regex string `json:"regex"`
		}
		if err := read(&p); err != nil {
			return nil, err
		}
		return RegexRule{Pattern: p.Regex}, nil
	case "field-equals":
		var p struct {
			FieldName string  `json:"fieldName"`
			Literal   *string `json:"literal"`
		}
		if err := read(&p); err != nil {
			return nil, err
		}
		return FieldEqualsRule{FieldName: p.FieldName, Literal: p.Literal}, nil
	case "field-absent":
		var p struct {
			FieldName string `json:"fieldName"`
		}
		if err := read(&p); err != nil {
			return nil, err
		}
		return FieldAbsentRule{FieldName: p.FieldName}, nil
	}
	return nil, fmt.Errorf("golem: unknown discriminator rule %q", d.Rule)
}

type wireUnionBranch struct {
	Tag           string            `json:"tag"`
	Body          json.RawMessage   `json:"body"`
	Discriminator wireDiscriminator `json:"discriminator"`
	Metadata      wireMetadata      `json:"metadata"`
}

type wireUnionType struct {
	wireBase
	Spec struct {
		Branches []wireUnionBranch `json:"branches"`
	} `json:"spec"`
}

type wireSecretType struct {
	wireBase
	Spec struct {
		Inner    json.RawMessage `json:"inner"`
		Category *string         `json:"category"`
	} `json:"spec"`
}

type wireQuotaTokenType struct {
	wireBase
	Spec struct {
		ResourceName *string `json:"resourceName"`
	} `json:"spec"`
}

type wirePermissionCardType struct {
	wireBase
	Spec struct {
		Polymorphic bool `json:"polymorphic"`
	} `json:"spec"`
}
