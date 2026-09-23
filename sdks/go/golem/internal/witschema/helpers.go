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
	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The generated bindings spell an optional value as a WIT option; core spells
// it as a pointer, which is what a Go program reads without a helper.

func optVal[T any](o witTypes.Option[T]) *T {
	if o.IsNone() {
		return nil
	}
	v := o.Some()
	return &v
}

func optSlice[T any](o witTypes.Option[[]T]) *[]T {
	if o.IsNone() {
		return nil
	}
	v := append([]T(nil), o.Some()...)
	return &v
}

func metadataToCore(m types.MetadataEnvelope) core.MetadataEnvelope {
	out := core.MetadataEnvelope{
		Doc:        optVal(m.Doc),
		Aliases:    append([]string(nil), m.Aliases...),
		Examples:   append([]string(nil), m.Examples...),
		Deprecated: optVal(m.Deprecated),
	}
	if m.Role.IsSome() {
		role := roleToCore(m.Role.Some())
		out.Role = &role
	}
	return out
}

func roleToCore(r types.Role) core.Role {
	switch r.Tag() {
	case types.RoleMultimodal:
		return core.RoleMultimodal
	case types.RoleUnstructuredText:
		return core.RoleUnstructuredText
	case types.RoleUnstructuredBinary:
		return core.RoleUnstructuredBinary
	}
	return core.Role(r.Other())
}

func numericToCore(o witTypes.Option[types.NumericRestrictions]) *core.NumericRestrictions {
	if o.IsNone() {
		return nil
	}
	r := o.Some()
	return &core.NumericRestrictions{
		Min:  boundToCore(r.Min),
		Max:  boundToCore(r.Max),
		Unit: optVal(r.Unit),
	}
}

// boundToCore keeps a numeric bound in whichever width can hold it, so nothing
// is lost passing through.
func boundToCore(o witTypes.Option[types.NumericBound]) *core.NumericBound {
	if o.IsNone() {
		return nil
	}
	b := o.Some()
	switch b.Tag() {
	case types.NumericBoundSigned:
		return &core.NumericBound{Kind: core.BoundSigned, Signed: b.Signed()}
	case types.NumericBoundUnsigned:
		return &core.NumericBound{Kind: core.BoundUnsigned, Unsigned: b.Unsigned()}
	case types.NumericBoundFloatBits:
		return &core.NumericBound{Kind: core.BoundFloatBits, FloatBits: b.FloatBits()}
	}
	return nil
}

func quantityToCore(o witTypes.Option[types.QuantityValue]) *core.QuantityValue {
	if o.IsNone() {
		return nil
	}
	q := o.Some()
	return &core.QuantityValue{Mantissa: q.Mantissa, Scale: q.Scale, Unit: q.Unit}
}

func discriminatorToCore(d types.DiscriminatorRule) core.DiscriminatorRule {
	switch d.Tag() {
	case types.DiscriminatorRulePrefix:
		return core.PrefixRule{Value: d.Prefix()}
	case types.DiscriminatorRuleSuffix:
		return core.SuffixRule{Value: d.Suffix()}
	case types.DiscriminatorRuleContains:
		return core.ContainsRule{Value: d.Contains()}
	case types.DiscriminatorRuleRegex:
		return core.RegexRule{Pattern: d.Regex()}
	case types.DiscriminatorRuleFieldEquals:
		f := d.FieldEquals()
		return core.FieldEqualsRule{FieldName: f.FieldName, Literal: optVal(f.Literal)}
	case types.DiscriminatorRuleFieldAbsent:
		return core.FieldAbsentRule{FieldName: d.FieldAbsent()}
	}
	return nil
}
