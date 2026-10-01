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
	"context"
	"fmt"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// The checked accessors a generated encoder or decoder for a named type is
// built from. Each one validates the shape it unpacks and names the type in
// its error, so a decoding failure says which of the client's types it was.

// RecordFields unpacks a record of exactly n fields.
func RecordFields(sv schema.SchemaValue, n int, typeName string) ([]schema.SchemaValue, error) {
	v, ok := sv.(schema.RecordValue)
	if !ok {
		return nil, Mismatch(typeName+" record", sv)
	}
	if len(v.Fields) != n {
		return nil, fmt.Errorf("golem: %s has %d fields, got %d", typeName, n, len(v.Fields))
	}
	return v.Fields, nil
}

// FieldError places a decoding failure at a field.
func FieldError(typeName, field string, err error) error {
	return fmt.Errorf("%s.%s: %w", typeName, field, err)
}

// EnumCase unpacks an enum case index and checks it is one of the n declared.
func EnumCase(sv schema.SchemaValue, n int, typeName string) (uint32, error) {
	v, ok := sv.(schema.EnumValue)
	if !ok {
		return 0, Mismatch(typeName+" enum", sv)
	}
	if int(v.Case) >= n {
		return 0, fmt.Errorf("golem: %s has %d cases, got case %d", typeName, n, v.Case)
	}
	return v.Case, nil
}

// FlagBits unpacks exactly n flags.
func FlagBits(sv schema.SchemaValue, n int, typeName string) ([]bool, error) {
	v, ok := sv.(schema.FlagsValue)
	if !ok {
		return nil, Mismatch(typeName+" flags", sv)
	}
	if len(v.Set) != n {
		return nil, fmt.Errorf("golem: %s has %d flags, got %d", typeName, n, len(v.Set))
	}
	return v.Set, nil
}

// VariantCase is a variant case carrying a payload.
func VariantCase(index uint32, payload schema.SchemaValue) schema.SchemaValue {
	return schema.VariantValue{Case: index, Payload: &payload}
}

// VariantUnit is a variant case carrying no payload.
func VariantUnit(index uint32) schema.SchemaValue {
	return schema.VariantValue{Case: index}
}

// VariantParts unpacks a variant into its case index and payload, which is nil
// for a case without one.
func VariantParts(sv schema.SchemaValue, typeName string) (uint32, *schema.SchemaValue, error) {
	v, ok := sv.(schema.VariantValue)
	if !ok {
		return 0, nil, Mismatch(typeName+" variant", sv)
	}
	return v.Case, v.Payload, nil
}

// CasePayload requires the payload a case declares.
func CasePayload(payload *schema.SchemaValue, typeName, caseName string) (schema.SchemaValue, error) {
	if payload == nil {
		return nil, fmt.Errorf("golem: %s case %q carries no payload", typeName, caseName)
	}
	return *payload, nil
}

// CaseNoPayload requires the absence of a payload for a case that declares none.
func CaseNoPayload(payload *schema.SchemaValue, typeName, caseName string) error {
	if payload != nil {
		return fmt.Errorf("golem: %s case %q declares no payload but one arrived", typeName, caseName)
	}
	return nil
}

// UnknownCase reports a case index the type does not declare.
func UnknownCase(typeName string, index uint32) error {
	return fmt.Errorf("golem: %s has no case %d", typeName, index)
}

// UnionBranch is a union value: the resolved branch's tag and its body.
func UnionBranch(tag string, body schema.SchemaValue) schema.SchemaValue {
	return schema.UnionValue{Tag: tag, Body: body}
}

// UnionParts unpacks a union into its branch tag and body.
func UnionParts(sv schema.SchemaValue, typeName string) (string, schema.SchemaValue, error) {
	v, ok := sv.(schema.UnionValue)
	if !ok {
		return "", nil, Mismatch(typeName+" union", sv)
	}
	return v.Tag, v.Body, nil
}

// UnknownBranch reports a union tag the type does not declare.
func UnknownBranch(typeName, tag string) error {
	return fmt.Errorf("golem: %s has no branch %q", typeName, tag)
}

// NotACase panics for a value that is not one of a sum type's cases. Only a nil
// interface can reach it, since the cases are sealed; encoding one is a
// programming error rather than a condition to report.
func NotACase(typeName string, v any) {
	panic(fmt.Sprintf("golem: %T is not a case of %s", v, typeName))
}

// Call invokes a method and decodes what it returns.
func Call[T any](ctx context.Context, a *Agent, method string, params schema.SchemaValue, f Decoder[T]) (T, error) {
	var zero T
	r, err := a.Invoke(ctx, method, params)
	if err != nil {
		return zero, err
	}
	if r.Value == nil {
		return zero, fmt.Errorf("golem: %s returned no value", method)
	}
	out, err := f(r.Value)
	if err != nil {
		return zero, fmt.Errorf("golem: decoding the result of %s: %w", method, err)
	}
	return out, nil
}
