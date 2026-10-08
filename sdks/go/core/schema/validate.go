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
	"math"
	"regexp"
	"strings"
	"unicode/utf8"
)

// Validate checks a value against the type it was built for: every node's
// kind, arity, case index and payload presence, and every restriction the type
// declares. A value read from the schema-free wire form is only trustworthy
// once this has accepted it.
//
// A stream node is accepted as it is: what it carries is a reference, and only
// whoever binds it knows whether that reference is valid.
func (r Ref) Validate(v SchemaValue) error {
	return r.validate(r.typ, v, "", 0)
}

const maxValidateDepth = 128

func (r Ref) validate(t SchemaType, v SchemaValue, path string, depth int) error {
	if depth > maxValidateDepth {
		return fmt.Errorf("%s: value nests deeper than %d levels", pathOrRoot(path), maxValidateDepth)
	}
	body, err := r.At(t).body()
	if err != nil {
		return err
	}
	switch b := body.(type) {
	case BoolType:
		_, err := expect[BoolValue](v, path, "bool")
		return err
	case S8Type:
		n, err := expect[S8Value](v, path, "s8")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, signedBound(int64(n.Value)), path)
	case S16Type:
		n, err := expect[S16Value](v, path, "s16")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, signedBound(int64(n.Value)), path)
	case S32Type:
		n, err := expect[S32Value](v, path, "s32")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, signedBound(int64(n.Value)), path)
	case S64Type:
		n, err := expect[S64Value](v, path, "s64")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, signedBound(n.Value), path)
	case U8Type:
		n, err := expect[U8Value](v, path, "u8")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, unsignedBound(uint64(n.Value)), path)
	case U16Type:
		n, err := expect[U16Value](v, path, "u16")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, unsignedBound(uint64(n.Value)), path)
	case U32Type:
		n, err := expect[U32Value](v, path, "u32")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, unsignedBound(uint64(n.Value)), path)
	case U64Type:
		n, err := expect[U64Value](v, path, "u64")
		if err != nil {
			return err
		}
		return checkNumeric(b.Restrictions, unsignedBound(n.Value), path)
	case F32Type:
		n, err := expect[F32Value](v, path, "f32")
		if err != nil {
			return err
		}
		return checkFloat(b.Restrictions, float64(n.Value), path)
	case F64Type:
		n, err := expect[F64Value](v, path, "f64")
		if err != nil {
			return err
		}
		return checkFloat(b.Restrictions, n.Value, path)
	case CharType:
		n, err := expect[CharValue](v, path, "char")
		if err != nil {
			return err
		}
		if !validCodePoint(n.Value) {
			return fmt.Errorf("%s: char %d is not a Unicode scalar value", pathOrRoot(path), n.Value)
		}
		return nil
	case StringType:
		n, err := expect[StringValue](v, path, "string")
		if err != nil {
			return err
		}
		return checkUTF8(n.Value, path)

	case RecordType:
		n, err := expect[RecordValue](v, path, "record")
		if err != nil {
			return err
		}
		if len(n.Fields) != len(b.Fields) {
			return fmt.Errorf("%s: record has %d fields, the type declares %d", pathOrRoot(path), len(n.Fields), len(b.Fields))
		}
		for i, f := range b.Fields {
			if err := r.validate(f.Body, n.Fields[i], child(path, f.Name), depth+1); err != nil {
				return err
			}
		}
		return nil
	case VariantType:
		n, err := expect[VariantValue](v, path, "variant")
		if err != nil {
			return err
		}
		if int(n.Case) >= len(b.Cases) {
			return fmt.Errorf("%s: variant has no case %d", pathOrRoot(path), n.Case)
		}
		c := b.Cases[n.Case]
		switch {
		case c.Payload == nil && n.Payload != nil:
			return fmt.Errorf("%s: variant case %q carries no payload", pathOrRoot(path), c.Name)
		case c.Payload != nil && n.Payload == nil:
			return fmt.Errorf("%s: variant case %q needs a payload", pathOrRoot(path), c.Name)
		case c.Payload != nil:
			return r.validate(*c.Payload, *n.Payload, child(path, c.Name), depth+1)
		}
		return nil
	case EnumType:
		n, err := expect[EnumValue](v, path, "enum")
		if err != nil {
			return err
		}
		if int(n.Case) >= len(b.Cases) {
			return fmt.Errorf("%s: enum has no case %d", pathOrRoot(path), n.Case)
		}
		return nil
	case FlagsType:
		n, err := expect[FlagsValue](v, path, "flags")
		if err != nil {
			return err
		}
		if len(n.Set) != len(b.Flags) {
			return fmt.Errorf("%s: flags carry %d bits, the type declares %d", pathOrRoot(path), len(n.Set), len(b.Flags))
		}
		return nil
	case TupleType:
		n, err := expect[TupleValue](v, path, "tuple")
		if err != nil {
			return err
		}
		if len(n.Elements) != len(b.Elements) {
			return fmt.Errorf("%s: tuple has %d elements, the type declares %d", pathOrRoot(path), len(n.Elements), len(b.Elements))
		}
		for i, e := range b.Elements {
			if err := r.validate(e, n.Elements[i], child(path, fmt.Sprintf("[%d]", i)), depth+1); err != nil {
				return err
			}
		}
		return nil
	case ListType:
		n, err := expect[ListValue](v, path, "list")
		if err != nil {
			return err
		}
		return r.validateItems(b.Element, n.Items, path, depth)
	case FixedListType:
		n, err := expect[FixedListValue](v, path, "fixed-list")
		if err != nil {
			return err
		}
		if len(n.Items) != int(b.Length) {
			return fmt.Errorf("%s: fixed list has %d elements, the type declares %d", pathOrRoot(path), len(n.Items), b.Length)
		}
		return r.validateItems(b.Element, n.Items, path, depth)
	case MapType:
		n, err := expect[MapValue](v, path, "map")
		if err != nil {
			return err
		}
		for i, entry := range n.Entries {
			at := child(path, fmt.Sprintf("[%d]", i))
			if err := r.validate(b.Key, entry.Key, child(at, "key"), depth+1); err != nil {
				return err
			}
			if err := r.validate(b.Value, entry.Value, child(at, "value"), depth+1); err != nil {
				return err
			}
		}
		return nil
	case OptionType:
		n, err := expect[OptionValue](v, path, "option")
		if err != nil {
			return err
		}
		if n.Value == nil {
			return nil
		}
		return r.validate(b.Inner, *n.Value, path, depth+1)
	case ResultType:
		n, err := expect[ResultValue](v, path, "result")
		if err != nil {
			return err
		}
		arm, declared := "ok", b.Ok
		if n.IsErr {
			arm, declared = "err", b.Err
		}
		switch {
		case declared == nil && n.Value != nil:
			return fmt.Errorf("%s: result %s carries no value", pathOrRoot(path), arm)
		case declared != nil && n.Value == nil:
			return fmt.Errorf("%s: result %s needs a value", pathOrRoot(path), arm)
		case declared != nil:
			return r.validate(*declared, *n.Value, child(path, arm), depth+1)
		}
		return nil

	case TextType:
		n, err := expect[TextValue](v, path, "text")
		if err != nil {
			return err
		}
		if err := checkUTF8(n.Text, path); err != nil {
			return err
		}
		return checkText(b.Restrictions, n, path)
	case BinaryType:
		n, err := expect[BinaryValue](v, path, "binary")
		if err != nil {
			return err
		}
		if n.MimeType != nil && !mimePattern.MatchString(*n.MimeType) {
			return fmt.Errorf("%s: %q is not a MIME type", pathOrRoot(path), *n.MimeType)
		}
		return checkBinary(b.Restrictions, n, path)
	case PathType:
		n, err := expect[PathValue](v, path, "path")
		if err != nil {
			return err
		}
		return checkPath(b.Spec, n.Value, path)
	case UrlType:
		n, err := expect[UrlValue](v, path, "url")
		if err != nil {
			return err
		}
		return checkURL(b.Restrictions, n.Value, path)
	case DatetimeType:
		_, err := expect[DatetimeValue](v, path, "datetime")
		return err
	case DurationType:
		_, err := expect[DurationValue](v, path, "duration")
		return err
	case UUIDType:
		_, err := expect[UUIDValue](v, path, "uuid")
		return err
	case QuantityType:
		n, err := expect[QuantityValueNode](v, path, "quantity")
		if err != nil {
			return err
		}
		return checkQuantity(b.Spec, n.Value, path)
	case UnionType:
		n, err := expect[UnionValue](v, path, "union")
		if err != nil {
			return err
		}
		for _, branch := range b.Branches {
			if branch.Tag != n.Tag {
				continue
			}
			if err := r.validate(branch.Body, n.Body, child(path, n.Tag), depth+1); err != nil {
				return err
			}
			if !r.discriminates(branch, n.Body) {
				return fmt.Errorf("%s: the body does not satisfy union branch %q's discriminator", pathOrRoot(path), n.Tag)
			}
			return nil
		}
		return fmt.Errorf("%s: union tag %q is not a declared branch", pathOrRoot(path), n.Tag)

	case StreamType:
		_, err := expect[StreamValue](v, path, "stream")
		return err
	case SecretType, QuotaTokenType, PermissionCardType, FutureType:
		return fmt.Errorf("%s: %T values cannot be validated outside the host", pathOrRoot(path), body)
	}
	return fmt.Errorf("%s: unsupported schema type %T", pathOrRoot(path), body)
}

func (r Ref) validateItems(elem SchemaType, items []SchemaValue, path string, depth int) error {
	for i, item := range items {
		if err := r.validate(elem, item, child(path, fmt.Sprintf("[%d]", i)), depth+1); err != nil {
			return err
		}
	}
	return nil
}

// discriminates reports whether a branch body satisfies the rule that selects
// its branch. String rules apply to a string or text body, field rules to a
// record body; a rule that cannot apply to the body's shape does not hold.
func (r Ref) discriminates(branch UnionBranch, v SchemaValue) bool {
	var text string
	hasText := false
	switch n := v.(type) {
	case StringValue:
		text, hasText = n.Value, true
	case TextValue:
		text, hasText = n.Text, true
	}
	switch d := branch.Discriminator.(type) {
	case PrefixRule:
		return hasText && strings.HasPrefix(text, d.Value)
	case SuffixRule:
		return hasText && strings.HasSuffix(text, d.Value)
	case ContainsRule:
		return hasText && strings.Contains(text, d.Value)
	case RegexRule:
		re, err := regexp.Compile(d.Pattern)
		return hasText && err == nil && re.MatchString(text)
	case FieldEqualsRule, FieldAbsentRule:
		record, ok := v.(RecordValue)
		if !ok {
			return false
		}
		body, err := r.At(branch.Body).body()
		if err != nil {
			return false
		}
		fields, ok := body.(RecordType)
		if !ok {
			return false
		}
		return fieldRuleHolds(d, fields, record)
	}
	return false
}

func fieldRuleHolds(rule DiscriminatorRule, t RecordType, v RecordValue) bool {
	find := func(name string) (SchemaValue, bool) {
		for i, f := range t.Fields {
			if f.Name == name && i < len(v.Fields) {
				return v.Fields[i], true
			}
		}
		return nil, false
	}
	switch d := rule.(type) {
	case FieldEqualsRule:
		field, ok := find(d.FieldName)
		if !ok {
			return false
		}
		if opt, isOpt := field.(OptionValue); isOpt {
			if opt.Value == nil {
				return false
			}
			field = *opt.Value
		}
		if d.Literal == nil {
			return true
		}
		switch n := field.(type) {
		case StringValue:
			return n.Value == *d.Literal
		case TextValue:
			return n.Text == *d.Literal
		}
		return false
	case FieldAbsentRule:
		field, ok := find(d.FieldName)
		if !ok {
			return true
		}
		opt, isOpt := field.(OptionValue)
		return isOpt && opt.Value == nil
	}
	return false
}

func checkFloat(rs *NumericRestrictions, f float64, path string) error {
	if rs == nil {
		return nil
	}
	if math.IsNaN(f) || math.IsInf(f, 0) {
		if rs.Min != nil || rs.Max != nil {
			return violation(path, "%v is outside every bound", f)
		}
		return nil
	}
	return checkNumeric(rs, floatBound(f), path)
}

func checkUTF8(s, path string) error {
	if !utf8.ValidString(s) {
		return fmt.Errorf("%s: not valid UTF-8", pathOrRoot(path))
	}
	return nil
}

var mimePattern = regexp.MustCompile(`^[A-Za-z0-9!#$&^_.+\-]+/[A-Za-z0-9!#$&^_.+\-]+$`)
