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

package witschema

import (
	"fmt"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// body converts one structural body.
func (c *converter) body(b types.SchemaTypeBody) (core.SchemaTypeBody, error) {
	switch b.Tag() {
	case types.SchemaTypeBodyRefType:
		i := b.RefType()
		if i < 0 || int(i) >= len(c.wit.Defs) {
			return nil, fmt.Errorf("golem: ref target %d is out of range (%d defs)", i, len(c.wit.Defs))
		}
		// Named by id, not followed: the definition is converted separately,
		// which is what makes a recursive type terminate.
		return core.RefType{Id: c.wit.Defs[i].Id}, nil

	case types.SchemaTypeBodyBoolType:
		return core.BoolType{}, nil
	case types.SchemaTypeBodyCharType:
		return core.CharType{}, nil
	case types.SchemaTypeBodyStringType:
		return core.StringType{}, nil
	case types.SchemaTypeBodyS8Type:
		return core.S8Type{Restrictions: numericToCore(b.S8Type())}, nil
	case types.SchemaTypeBodyS16Type:
		return core.S16Type{Restrictions: numericToCore(b.S16Type())}, nil
	case types.SchemaTypeBodyS32Type:
		return core.S32Type{Restrictions: numericToCore(b.S32Type())}, nil
	case types.SchemaTypeBodyS64Type:
		return core.S64Type{Restrictions: numericToCore(b.S64Type())}, nil
	case types.SchemaTypeBodyU8Type:
		return core.U8Type{Restrictions: numericToCore(b.U8Type())}, nil
	case types.SchemaTypeBodyU16Type:
		return core.U16Type{Restrictions: numericToCore(b.U16Type())}, nil
	case types.SchemaTypeBodyU32Type:
		return core.U32Type{Restrictions: numericToCore(b.U32Type())}, nil
	case types.SchemaTypeBodyU64Type:
		return core.U64Type{Restrictions: numericToCore(b.U64Type())}, nil
	case types.SchemaTypeBodyF32Type:
		return core.F32Type{Restrictions: numericToCore(b.F32Type())}, nil
	case types.SchemaTypeBodyF64Type:
		return core.F64Type{Restrictions: numericToCore(b.F64Type())}, nil

	case types.SchemaTypeBodyRecordType:
		fields := make([]core.NamedField, 0, len(b.RecordType()))
		for _, f := range b.RecordType() {
			body, err := c.child(f.Body)
			if err != nil {
				return nil, err
			}
			fields = append(fields, core.NamedField{
				Name: f.Name, Body: body, Metadata: metadataToCore(f.Metadata),
			})
		}
		return core.RecordType{Fields: fields}, nil

	case types.SchemaTypeBodyVariantType:
		cases := make([]core.VariantCase, 0, len(b.VariantType()))
		for _, v := range b.VariantType() {
			payload, err := c.optChild(v.Payload)
			if err != nil {
				return nil, err
			}
			cases = append(cases, core.VariantCase{
				Name: v.Name, Payload: payload, Metadata: metadataToCore(v.Metadata),
			})
		}
		return core.VariantType{Cases: cases}, nil

	case types.SchemaTypeBodyEnumType:
		return core.EnumType{Cases: append([]string(nil), b.EnumType()...)}, nil
	case types.SchemaTypeBodyFlagsType:
		return core.FlagsType{Flags: append([]string(nil), b.FlagsType()...)}, nil

	case types.SchemaTypeBodyTupleType:
		elems := make([]core.SchemaType, 0, len(b.TupleType()))
		for _, e := range b.TupleType() {
			t, err := c.child(e)
			if err != nil {
				return nil, err
			}
			elems = append(elems, t)
		}
		return core.TupleType{Elements: elems}, nil

	case types.SchemaTypeBodyListType:
		t, err := c.child(b.ListType())
		return core.ListType{Element: t}, err

	case types.SchemaTypeBodyFixedListType:
		spec := b.FixedListType()
		t, err := c.child(spec.Element)
		return core.FixedListType{Element: t, Length: spec.Length}, err

	case types.SchemaTypeBodyMapType:
		spec := b.MapType()
		k, err := c.child(spec.Key)
		if err != nil {
			return nil, err
		}
		v, err := c.child(spec.Value)
		return core.MapType{Key: k, Value: v}, err

	case types.SchemaTypeBodyOptionType:
		t, err := c.child(b.OptionType())
		return core.OptionType{Inner: t}, err

	case types.SchemaTypeBodyResultType:
		spec := b.ResultType()
		ok, err := c.optChild(spec.Ok)
		if err != nil {
			return nil, err
		}
		bad, err := c.optChild(spec.Err)
		return core.ResultType{Ok: ok, Err: bad}, err

	case types.SchemaTypeBodyTextType:
		r := b.TextType()
		return core.TextType{Restrictions: core.TextRestrictions{
			Languages: optSlice(r.Languages),
			MinLength: optVal(r.MinLength),
			MaxLength: optVal(r.MaxLength),
			Regex:     optVal(r.Regex),
		}}, nil

	case types.SchemaTypeBodyBinaryType:
		r := b.BinaryType()
		return core.BinaryType{Restrictions: core.BinaryRestrictions{
			MimeTypes: optSlice(r.MimeTypes),
			MinBytes:  optVal(r.MinBytes),
			MaxBytes:  optVal(r.MaxBytes),
		}}, nil

	case types.SchemaTypeBodyPathType:
		s := b.PathType()
		return core.PathType{Spec: core.PathSpec{
			Direction:         core.PathDirection(s.Direction),
			Kind:              core.PathKind(s.Kind),
			AllowedMimeTypes:  optSlice(s.AllowedMimeTypes),
			AllowedExtensions: optSlice(s.AllowedExtensions),
		}}, nil

	case types.SchemaTypeBodyUrlType:
		r := b.UrlType()
		return core.UrlType{Restrictions: core.UrlRestrictions{
			AllowedSchemes: optSlice(r.AllowedSchemes),
			AllowedHosts:   optSlice(r.AllowedHosts),
		}}, nil

	case types.SchemaTypeBodyDatetimeType:
		return core.DatetimeType{}, nil
	case types.SchemaTypeBodyDurationType:
		return core.DurationType{}, nil

	case types.SchemaTypeBodyQuantityType:
		s := b.QuantityType()
		return core.QuantityType{Spec: core.QuantitySpec{
			BaseUnit:        s.BaseUnit,
			AllowedSuffixes: append([]string(nil), s.AllowedSuffixes...),
			Min:             quantityToCore(s.Min),
			Max:             quantityToCore(s.Max),
		}}, nil

	case types.SchemaTypeBodyUnionType:
		branches := make([]core.UnionBranch, 0, len(b.UnionType().Branches))
		for _, br := range b.UnionType().Branches {
			body, err := c.child(br.Body)
			if err != nil {
				return nil, err
			}
			branches = append(branches, core.UnionBranch{
				Tag:           br.Tag,
				Body:          body,
				Discriminator: discriminatorToCore(br.Discriminator),
				Metadata:      metadataToCore(br.Metadata),
			})
		}
		return core.UnionType{Branches: branches}, nil

	case types.SchemaTypeBodySecretType:
		s := b.SecretType()
		inner, err := c.child(s.Inner)
		return core.SecretType{Inner: inner, Category: optVal(s.Category)}, err

	case types.SchemaTypeBodyQuotaTokenType:
		return core.QuotaTokenType{ResourceName: optVal(b.QuotaTokenType().ResourceName)}, nil
	case types.SchemaTypeBodyPermissionCardType:
		return core.PermissionCardType{Polymorphic: b.PermissionCardType().Polymorphic}, nil

	case types.SchemaTypeBodyFutureType:
		item, err := c.optChild(b.FutureType())
		return core.FutureType{Item: item}, err
	case types.SchemaTypeBodyStreamType:
		item, err := c.optChild(b.StreamType())
		return core.StreamType{Item: item}, err
	}
	// Go switches are not exhaustive, so a WIT case added later would fall
	// through silently. Saying so is better than converting it to nothing.
	return nil, fmt.Errorf("golem: unknown schema type (tag %d); the SDK's bindings may be out of date", b.Tag())
}

// witBodyTagCount pins how many type cases the bindings declare. Bump it
// deliberately, with the case added above — see TestEveryWitBodyTagConverts.
const witBodyTagCount = 37
