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
	"fmt"
	"strings"

	witTypes "go.bytecodealliance.org/pkg/wit/types"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Issue is one reason a value does not match its schema. Path locates it the
// way a user would read it — `items[2].name`, `result.err`, `flags` — so a
// caller can report which part of its input was wrong.
type Issue struct {
	Path    string
	Message string
}

func (i Issue) String() string {
	if i.Path == "" {
		return i.Message
	}
	return i.Path + ": " + i.Message
}

// ValidationError reports every way a value failed to match a schema. It is
// returned whole rather than as the first failure, because a caller fixing
// generated input wants the full list.
type ValidationError struct {
	Issues []Issue
}

func (e *ValidationError) Error() string {
	switch len(e.Issues) {
	case 0:
		return "golem: schema validation failed"
	case 1:
		return "golem: schema validation failed: " + e.Issues[0].String()
	default:
		parts := make([]string, 0, len(e.Issues))
		for _, issue := range e.Issues {
			parts = append(parts, issue.String())
		}
		return fmt.Sprintf("golem: schema validation failed (%d issues): %s",
			len(e.Issues), strings.Join(parts, "; "))
	}
}

// Validate checks that a value tree matches this schema.
//
// It is structural: every node must have the shape its type declares, indices
// must be in range, records must carry exactly their fields, and variant,
// enum, union and flags selectors must name a declared case. Declared
// restrictions (lengths, numeric bounds, allowed MIME types) are checked where
// the schema states them.
func (r Ref) Validate(value types.SchemaValueTree) error {
	v := &validator{ref: r, value: value}
	v.check(r.root, value.Root, "")
	if len(v.issues) == 0 {
		return nil
	}
	return &ValidationError{Issues: v.issues}
}

type validator struct {
	ref    Ref
	value  types.SchemaValueTree
	issues []Issue
	// depth guards against a value tree whose indices form a cycle; the schema
	// side is already cycle-safe through named defs.
	depth int
}

const maxValueDepth = 512

func (v *validator) fail(path, format string, args ...any) {
	v.issues = append(v.issues, Issue{Path: path, Message: fmt.Sprintf(format, args...)})
}

func (v *validator) node(idx int32, path string) (types.SchemaValueNode, bool) {
	if idx < 0 || int(idx) >= len(v.value.ValueNodes) {
		v.fail(path, "value node index %d is out of range (%d nodes)", idx, len(v.value.ValueNodes))
		return types.SchemaValueNode{}, false
	}
	return v.value.ValueNodes[idx], true
}

func child(path, segment string) string {
	if path == "" {
		return segment
	}
	if strings.HasPrefix(segment, "[") {
		return path + segment
	}
	return path + "." + segment
}

// check validates the value at valueIdx against the type at typeIdx.
func (v *validator) check(typeIdx, valueIdx int32, path string) {
	v.depth++
	defer func() { v.depth-- }()
	if v.depth > maxValueDepth {
		v.fail(path, "value nesting exceeds %d levels", maxValueDepth)
		return
	}

	body, _, err := v.ref.node(typeIdx)
	if err != nil {
		v.fail(path, "%s", err)
		return
	}
	node, ok := v.node(valueIdx, path)
	if !ok {
		return
	}

	switch body.Tag() {
	case types.SchemaTypeBodyBoolType:
		v.expect(node, types.SchemaValueNodeBoolValue, "bool", path)
	case types.SchemaTypeBodyS8Type:
		v.expect(node, types.SchemaValueNodeS8Value, "s8", path)
	case types.SchemaTypeBodyS16Type:
		v.expect(node, types.SchemaValueNodeS16Value, "s16", path)
	case types.SchemaTypeBodyS32Type:
		v.expect(node, types.SchemaValueNodeS32Value, "s32", path)
	case types.SchemaTypeBodyS64Type:
		v.expect(node, types.SchemaValueNodeS64Value, "s64", path)
	case types.SchemaTypeBodyU8Type:
		v.expect(node, types.SchemaValueNodeU8Value, "u8", path)
	case types.SchemaTypeBodyU16Type:
		v.expect(node, types.SchemaValueNodeU16Value, "u16", path)
	case types.SchemaTypeBodyU32Type:
		v.expect(node, types.SchemaValueNodeU32Value, "u32", path)
	case types.SchemaTypeBodyU64Type:
		v.expect(node, types.SchemaValueNodeU64Value, "u64", path)
	case types.SchemaTypeBodyF32Type:
		v.expect(node, types.SchemaValueNodeF32Value, "f32", path)
	case types.SchemaTypeBodyF64Type:
		v.expect(node, types.SchemaValueNodeF64Value, "f64", path)
	case types.SchemaTypeBodyCharType:
		v.expect(node, types.SchemaValueNodeCharValue, "char", path)
	case types.SchemaTypeBodyStringType:
		v.expect(node, types.SchemaValueNodeStringValue, "string", path)
	case types.SchemaTypeBodyPathType:
		v.expect(node, types.SchemaValueNodePathValue, "path", path)
	case types.SchemaTypeBodyUrlType:
		v.expect(node, types.SchemaValueNodeUrlValue, "url", path)
	case types.SchemaTypeBodyDatetimeType:
		if v.expect(node, types.SchemaValueNodeDatetimeValue, "datetime", path) {
			if ns := node.DatetimeValue().Nanoseconds; ns >= 1_000_000_000 {
				v.fail(path, "datetime nanoseconds %d is not below 1e9", ns)
			}
		}
	case types.SchemaTypeBodyDurationType:
		v.expect(node, types.SchemaValueNodeDurationValue, "duration", path)
	case types.SchemaTypeBodyQuotaTokenType:
		v.expect(node, types.SchemaValueNodeQuotaTokenHandle, "quota-token", path)
	case types.SchemaTypeBodyPermissionCardType:
		v.expect(node, types.SchemaValueNodePermissionCardHandle, "permission-card", path)

	case types.SchemaTypeBodyTextType:
		if v.expect(node, types.SchemaValueNodeTextValue, "text", path) {
			v.checkTextRestrictions(body.TextType(), node.TextValue(), path)
		}
	case types.SchemaTypeBodyBinaryType:
		if v.expect(node, types.SchemaValueNodeBinaryValue, "binary", path) {
			v.checkBinaryRestrictions(body.BinaryType(), node.BinaryValue(), path)
		}
	case types.SchemaTypeBodyQuantityType:
		v.expect(node, types.SchemaValueNodeQuantityValueNode, "quantity", path)

	case types.SchemaTypeBodyEnumType:
		if v.expect(node, types.SchemaValueNodeEnumValue, "enum", path) {
			cases := body.EnumType()
			if sel := node.EnumValue(); int(sel) >= len(cases) {
				v.fail(path, "enum case %d is out of range (%d cases)", sel, len(cases))
			}
		}
	case types.SchemaTypeBodyFlagsType:
		if v.expect(node, types.SchemaValueNodeFlagsValue, "flags", path) {
			// One bool per declared flag, in declaration order.
			if declared, set := body.FlagsType(), node.FlagsValue(); len(set) != len(declared) {
				v.fail(path, "flags carries %d bit(s), schema declares %d", len(set), len(declared))
			}
		}
	case types.SchemaTypeBodyRecordType:
		if v.expect(node, types.SchemaValueNodeRecordValue, "record", path) {
			fields := body.RecordType()
			values := node.RecordValue()
			if len(values) != len(fields) {
				v.fail(path, "record has %d field(s), schema declares %d", len(values), len(fields))
				return
			}
			for i, f := range fields {
				v.check(f.Body, values[i], child(path, f.Name))
			}
		}
	case types.SchemaTypeBodyVariantType:
		if v.expect(node, types.SchemaValueNodeVariantValue, "variant", path) {
			cases := body.VariantType()
			payload := node.VariantValue()
			if int(payload.Case) >= len(cases) {
				v.fail(path, "variant case %d is out of range (%d cases)", payload.Case, len(cases))
				return
			}
			declared := cases[payload.Case]
			casePath := child(path, declared.Name)
			switch {
			case declared.Payload.IsSome() && payload.Payload.IsSome():
				v.check(declared.Payload.Some(), payload.Payload.Some(), casePath)
			case declared.Payload.IsSome():
				v.fail(casePath, "case carries no payload but the schema declares one")
			case payload.Payload.IsSome():
				v.fail(casePath, "case carries a payload but the schema declares none")
			}
		}
	case types.SchemaTypeBodyUnionType:
		if v.expect(node, types.SchemaValueNodeUnionValue, "union", path) {
			payload := node.UnionValue()
			branches := body.UnionType().Branches
			idx := -1
			for i, b := range branches {
				if b.Tag == payload.Tag {
					idx = i
					break
				}
			}
			if idx < 0 {
				v.fail(path, "union tag %q is not a declared branch", payload.Tag)
				return
			}
			v.check(branches[idx].Body, payload.Body, child(path, payload.Tag))
		}
	case types.SchemaTypeBodyTupleType:
		if v.expect(node, types.SchemaValueNodeTupleValue, "tuple", path) {
			elems := body.TupleType()
			values := node.TupleValue()
			if len(values) != len(elems) {
				v.fail(path, "tuple has %d element(s), schema declares %d", len(values), len(elems))
				return
			}
			for i, elem := range elems {
				v.check(elem, values[i], child(path, fmt.Sprintf("[%d]", i)))
			}
		}
	case types.SchemaTypeBodyListType:
		if v.expect(node, types.SchemaValueNodeListValue, "list", path) {
			elem := body.ListType()
			for i, item := range node.ListValue() {
				v.check(elem, item, child(path, fmt.Sprintf("[%d]", i)))
			}
		}
	case types.SchemaTypeBodyFixedListType:
		if v.expect(node, types.SchemaValueNodeFixedListValue, "fixed-list", path) {
			spec := body.FixedListType()
			values := node.FixedListValue()
			if uint32(len(values)) != spec.Length {
				v.fail(path, "fixed list has %d element(s), schema declares %d", len(values), spec.Length)
				return
			}
			for i, item := range values {
				v.check(spec.Element, item, child(path, fmt.Sprintf("[%d]", i)))
			}
		}
	case types.SchemaTypeBodyMapType:
		if v.expect(node, types.SchemaValueNodeMapValue, "map", path) {
			spec := body.MapType()
			for i, entry := range node.MapValue() {
				v.check(spec.Key, entry.Key, child(path, fmt.Sprintf("[%d].key", i)))
				v.check(spec.Value, entry.Value, child(path, fmt.Sprintf("[%d].value", i)))
			}
		}
	case types.SchemaTypeBodyOptionType:
		if v.expect(node, types.SchemaValueNodeOptionValue, "option", path) {
			if inner := node.OptionValue(); inner.IsSome() {
				v.check(body.OptionType(), inner.Some(), path)
			}
		}
	case types.SchemaTypeBodyResultType:
		if v.expect(node, types.SchemaValueNodeResultValue, "result", path) {
			spec := body.ResultType()
			payload := node.ResultValue()
			switch payload.Tag() {
			case types.ResultValuePayloadOkValue:
				v.checkResultArm(spec.Ok, payload.OkValue(), child(path, "ok"), "ok")
			default:
				v.checkResultArm(spec.Err, payload.ErrValue(), child(path, "err"), "err")
			}
		}
	case types.SchemaTypeBodySecretType:
		v.expect(node, types.SchemaValueNodeSecretValue, "secret", path)
	case types.SchemaTypeBodyStreamType:
		v.expect(node, types.SchemaValueNodeStreamValue, "stream", path)
	case types.SchemaTypeBodyFutureType:
		// Futures are a parse-only stub in the schema model: there is no value
		// node for them, so any value here is a schema-construction error.
		v.fail(path, "future types carry no value")
	default:
		v.fail(path, "unsupported schema type (tag %d)", body.Tag())
	}
}

// checkResultArm validates one arm of a result, where both the schema and the
// value may independently carry or omit a payload.
func (v *validator) checkResultArm(declared, carried witTypes.Option[int32], path, arm string) {
	switch {
	case declared.IsSome() && carried.IsSome():
		v.check(declared.Some(), carried.Some(), path)
	case declared.IsSome():
		v.fail(path, "%s arm carries no payload but the schema declares one", arm)
	case carried.IsSome():
		v.fail(path, "%s arm carries a payload but the schema declares none", arm)
	}
}

// expect reports a mismatch between the value node's kind and the schema's.
func (v *validator) expect(node types.SchemaValueNode, want uint8, label, path string) bool {
	if node.Tag() == want {
		return true
	}
	v.fail(path, "expected a %s value, found %s", label, valueKindName(node.Tag()))
	return false
}

func (v *validator) checkTextRestrictions(r types.TextRestrictions, payload types.TextValuePayload, path string) {
	length := uint32(len([]rune(payload.Text)))
	if r.MinLength.IsSome() && length < r.MinLength.Some() {
		v.fail(path, "text is %d character(s), minimum is %d", length, r.MinLength.Some())
	}
	if r.MaxLength.IsSome() && length > r.MaxLength.Some() {
		v.fail(path, "text is %d character(s), maximum is %d", length, r.MaxLength.Some())
	}
	if r.Languages.IsSome() && payload.Language.IsSome() {
		if !containsString(r.Languages.Some(), payload.Language.Some()) {
			v.fail(path, "language %q is not among the allowed languages", payload.Language.Some())
		}
	}
}

func (v *validator) checkBinaryRestrictions(r types.BinaryRestrictions, payload types.BinaryValuePayload, path string) {
	size := uint32(len(payload.Bytes))
	if r.MinBytes.IsSome() && size < r.MinBytes.Some() {
		v.fail(path, "binary is %d byte(s), minimum is %d", size, r.MinBytes.Some())
	}
	if r.MaxBytes.IsSome() && size > r.MaxBytes.Some() {
		v.fail(path, "binary is %d byte(s), maximum is %d", size, r.MaxBytes.Some())
	}
	if r.MimeTypes.IsSome() && payload.MimeType.IsSome() {
		if !containsString(r.MimeTypes.Some(), payload.MimeType.Some()) {
			v.fail(path, "mime type %q is not among the allowed types", payload.MimeType.Some())
		}
	}
}

func containsString(haystack []string, needle string) bool {
	for _, s := range haystack {
		if s == needle {
			return true
		}
	}
	return false
}

// valueKindName names a value node kind for error messages.
func valueKindName(tag uint8) string {
	switch tag {
	case types.SchemaValueNodeBoolValue:
		return "bool"
	case types.SchemaValueNodeS8Value, types.SchemaValueNodeS16Value,
		types.SchemaValueNodeS32Value, types.SchemaValueNodeS64Value:
		return "signed integer"
	case types.SchemaValueNodeU8Value, types.SchemaValueNodeU16Value,
		types.SchemaValueNodeU32Value, types.SchemaValueNodeU64Value:
		return "unsigned integer"
	case types.SchemaValueNodeF32Value, types.SchemaValueNodeF64Value:
		return "float"
	case types.SchemaValueNodeCharValue:
		return "char"
	case types.SchemaValueNodeStringValue:
		return "string"
	case types.SchemaValueNodeRecordValue:
		return "record"
	case types.SchemaValueNodeVariantValue:
		return "variant"
	case types.SchemaValueNodeEnumValue:
		return "enum"
	case types.SchemaValueNodeFlagsValue:
		return "flags"
	case types.SchemaValueNodeTupleValue:
		return "tuple"
	case types.SchemaValueNodeListValue:
		return "list"
	case types.SchemaValueNodeFixedListValue:
		return "fixed list"
	case types.SchemaValueNodeMapValue:
		return "map"
	case types.SchemaValueNodeOptionValue:
		return "option"
	case types.SchemaValueNodeResultValue:
		return "result"
	case types.SchemaValueNodeTextValue:
		return "text"
	case types.SchemaValueNodeBinaryValue:
		return "binary"
	case types.SchemaValueNodePathValue:
		return "path"
	case types.SchemaValueNodeUrlValue:
		return "url"
	case types.SchemaValueNodeDatetimeValue:
		return "datetime"
	case types.SchemaValueNodeDurationValue:
		return "duration"
	case types.SchemaValueNodeQuantityValueNode:
		return "quantity"
	case types.SchemaValueNodeUnionValue:
		return "union"
	case types.SchemaValueNodeSecretValue:
		return "secret"
	case types.SchemaValueNodeQuotaTokenHandle:
		return "quota-token"
	case types.SchemaValueNodePermissionCardHandle:
		return "permission-card"
	case types.SchemaValueNodeStreamValue:
		return "stream"
	default:
		return fmt.Sprintf("value kind %d", tag)
	}
}
