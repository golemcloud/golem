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

package golem

import (
	"fmt"
	"math"
	"reflect"
	"regexp"
	"slices"
	"strconv"
	"strings"

	"github.com/golemcloud/golem/sdks/go/core/values"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Schema restrictions.
//
// A restriction narrows the values a field or a tool argument accepts, and is
// published in its schema: callers, the CLI and generated clients all see it,
// and the platform enforces it before the agent runs.
//
// On a method's parameters or a record's fields, restrictions are a struct tag:
//
//	type ResizeIn struct {
//		Width  uint32       `golem:"min=1,max=4096"`
//		Format golem.Text   `golem:"languages=en|de,maxLength=40"`
//		Photo  golem.Binary `golem:"mime=image/png|image/jpeg,maxBytes=1048576"`
//		Source golem.URL    `golem:"schemes=https"`
//		Out    golem.Path   `golem:"direction=output,kind=file,extensions=png"`
//		Name   golem.Text   `golem:"minLength=1,regex=^[a-z][a-z0-9-]*$"`
//	}
//
// Lists are separated by |. A regex runs to the end of the tag, so it may
// contain commas; put it last. A tool argument takes the same tag, or setters
// on its binding: s.Option(&a.Width).Range(1, 4096).
//
// Which keys apply depends on the field's type: min, max and unit on numbers
// (min and max also on a quantity, as "1.5kg" or in its base unit);
// languages, minLength, maxLength and regex on golem.Text; mime, minBytes and
// maxBytes on golem.Binary; direction (input, output, inout), kind (file,
// directory, any), mime and extensions on golem.Path; schemes and hosts on
// golem.URL. A plain string carries no restrictions — use golem.Text. An
// optional field is restricted through to its value, and a list through to its
// elements.

// PathDirection is which way a path argument's file flows.
type PathDirection uint8

const (
	PathInput PathDirection = iota
	PathOutput
	PathInOut
)

// PathKind is what a path argument names.
type PathKind uint8

const (
	PathFile PathKind = iota
	PathDirectory
	PathAny
)

// restriction is what a struct tag or a tool argument's setters declared.
// Bounds stay text until the type they bound is known.
type restriction struct {
	min, max           *string
	unit               *string
	languages          *[]string
	minLength          *uint32
	maxLength          *uint32
	regex              *string
	mime               *[]string
	minBytes, maxBytes *uint32
	direction          *PathDirection
	kind               *PathKind
	extensions         *[]string
	schemes, hosts     *[]string
}

// parseRestrictionTag reads a `golem:"…"` tag.
func parseRestrictionTag(tag string) (*restriction, error) {
	r := &restriction{}
	rest := tag
	for rest != "" {
		var part string
		if strings.HasPrefix(rest, "regex=") {
			part, rest = rest, ""
		} else if i := strings.IndexByte(rest, ','); i >= 0 {
			part, rest = rest[:i], rest[i+1:]
		} else {
			part, rest = rest, ""
		}
		key, value, ok := strings.Cut(part, "=")
		if !ok || value == "" {
			return nil, fmt.Errorf("restriction %q must be key=value", part)
		}
		list := func() *[]string {
			items := strings.Split(value, "|")
			return &items
		}
		u32 := func() (*uint32, error) {
			n, err := strconv.ParseUint(value, 10, 32)
			if err != nil {
				return nil, fmt.Errorf("%s must be a non-negative integer, got %q", key, value)
			}
			v := uint32(n)
			return &v, nil
		}
		var err error
		switch key {
		case "min":
			r.min = &value
		case "max":
			r.max = &value
		case "unit":
			r.unit = &value
		case "languages":
			r.languages = list()
		case "minLength":
			r.minLength, err = u32()
		case "maxLength":
			r.maxLength, err = u32()
		case "regex":
			if _, err = regexp.Compile(value); err != nil {
				err = fmt.Errorf("regex %q: %w", value, err)
			}
			r.regex = &value
		case "mime":
			r.mime = list()
		case "minBytes":
			r.minBytes, err = u32()
		case "maxBytes":
			r.maxBytes, err = u32()
		case "direction":
			var d PathDirection
			switch value {
			case "input":
				d = PathInput
			case "output":
				d = PathOutput
			case "inout":
				d = PathInOut
			default:
				err = fmt.Errorf("direction must be input, output or inout, got %q", value)
			}
			r.direction = &d
		case "kind":
			var k PathKind
			switch value {
			case "file":
				k = PathFile
			case "directory":
				k = PathDirectory
			case "any":
				k = PathAny
			default:
				err = fmt.Errorf("kind must be file, directory or any, got %q", value)
			}
			r.kind = &k
		case "extensions":
			r.extensions = list()
		case "schemes":
			r.schemes = list()
		case "hosts":
			r.hosts = list()
		default:
			err = fmt.Errorf("unknown restriction %q", key)
		}
		if err != nil {
			return nil, err
		}
	}
	return r, nil
}

// apply restricts a type node's body, refusing a restriction its kind has no
// place for.
func (r *restriction) apply(body types.SchemaTypeBody) (types.SchemaTypeBody, error) {
	var allowed []string
	check := func(kind string) error {
		set := map[string]bool{
			"min": r.min != nil, "max": r.max != nil, "unit": r.unit != nil,
			"languages": r.languages != nil, "minLength": r.minLength != nil, "maxLength": r.maxLength != nil,
			"regex": r.regex != nil, "mime": r.mime != nil, "minBytes": r.minBytes != nil,
			"maxBytes": r.maxBytes != nil, "direction": r.direction != nil, "kind": r.kind != nil,
			"extensions": r.extensions != nil, "schemes": r.schemes != nil, "hosts": r.hosts != nil,
		}
		for key, isSet := range set {
			if isSet && !slices.Contains(allowed, key) {
				hint := ""
				if kind == "string" {
					hint = "; use golem.Text for a restricted string"
				}
				return fmt.Errorf("restriction %s does not apply to %s%s", key, kind, hint)
			}
		}
		return nil
	}
	tag := body.Tag()
	switch {
	case tag >= types.SchemaTypeBodyS8Type && tag <= types.SchemaTypeBodyF64Type:
		allowed = []string{"min", "max", "unit"}
		if err := check("a number"); err != nil {
			return body, err
		}
		rs := types.NumericRestrictions{
			Min:  witTypes.None[types.NumericBound](),
			Max:  witTypes.None[types.NumericBound](),
			Unit: optionOf(r.unit),
		}
		for _, side := range []struct {
			text *string
			dst  *witTypes.Option[types.NumericBound]
		}{{r.min, &rs.Min}, {r.max, &rs.Max}} {
			if side.text == nil {
				continue
			}
			bound, err := numericBound(tag, *side.text)
			if err != nil {
				return body, err
			}
			*side.dst = witTypes.Some(bound)
		}
		return numericBody(tag, witTypes.Some(rs)), nil
	case tag == types.SchemaTypeBodyTextType:
		allowed = []string{"languages", "minLength", "maxLength", "regex"}
		if err := check("text"); err != nil {
			return body, err
		}
		return types.MakeSchemaTypeBodyTextType(types.TextRestrictions{
			Languages: optionOf(r.languages),
			MinLength: optionOf(r.minLength),
			MaxLength: optionOf(r.maxLength),
			Regex:     optionOf(r.regex),
		}), nil
	case tag == types.SchemaTypeBodyBinaryType:
		allowed = []string{"mime", "minBytes", "maxBytes"}
		if err := check("binary"); err != nil {
			return body, err
		}
		return types.MakeSchemaTypeBodyBinaryType(types.BinaryRestrictions{
			MimeTypes: optionOf(r.mime),
			MinBytes:  optionOf(r.minBytes),
			MaxBytes:  optionOf(r.maxBytes),
		}), nil
	case tag == types.SchemaTypeBodyPathType:
		allowed = []string{"direction", "kind", "mime", "extensions"}
		if err := check("a path"); err != nil {
			return body, err
		}
		spec := body.PathType()
		if r.direction != nil {
			spec.Direction = types.PathDirection(*r.direction)
		}
		if r.kind != nil {
			spec.Kind = types.PathKind(*r.kind)
		}
		spec.AllowedMimeTypes = optionOf(r.mime)
		spec.AllowedExtensions = optionOf(r.extensions)
		return types.MakeSchemaTypeBodyPathType(spec), nil
	case tag == types.SchemaTypeBodyUrlType:
		allowed = []string{"schemes", "hosts"}
		if err := check("a URL"); err != nil {
			return body, err
		}
		return types.MakeSchemaTypeBodyUrlType(types.UrlRestrictions{
			AllowedSchemes: optionOf(r.schemes),
			AllowedHosts:   optionOf(r.hosts),
		}), nil
	case tag == types.SchemaTypeBodyQuantityType:
		allowed = []string{"min", "max"}
		if err := check("a quantity"); err != nil {
			return body, err
		}
		spec := body.QuantityType()
		for _, side := range []struct {
			text *string
			dst  *witTypes.Option[types.QuantityValue]
		}{{r.min, &spec.Min}, {r.max, &spec.Max}} {
			if side.text == nil {
				continue
			}
			q, err := parseQuantityBound(*side.text, spec.BaseUnit)
			if err != nil {
				return body, err
			}
			*side.dst = witTypes.Some(q)
		}
		return types.MakeSchemaTypeBodyQuantityType(spec), nil
	case tag == types.SchemaTypeBodyStringType:
		return body, check("string")
	}
	return body, check(fmt.Sprintf("this type (schema node %d)", tag))
}

func optionOf[T any](p *T) witTypes.Option[T] {
	if p == nil {
		return witTypes.None[T]()
	}
	return witTypes.Some(*p)
}

func numericBound(tag uint8, text string) (types.NumericBound, error) {
	switch tag {
	case types.SchemaTypeBodyS8Type, types.SchemaTypeBodyS16Type, types.SchemaTypeBodyS32Type, types.SchemaTypeBodyS64Type:
		n, err := strconv.ParseInt(text, 10, 64)
		if err != nil {
			return types.NumericBound{}, fmt.Errorf("bound %q is not an integer", text)
		}
		return types.MakeNumericBoundSigned(n), nil
	case types.SchemaTypeBodyF32Type, types.SchemaTypeBodyF64Type:
		f, err := strconv.ParseFloat(text, 64)
		if err != nil || math.IsNaN(f) {
			return types.NumericBound{}, fmt.Errorf("bound %q is not a number", text)
		}
		return types.MakeNumericBoundFloatBits(math.Float64bits(f)), nil
	}
	n, err := strconv.ParseUint(text, 10, 64)
	if err != nil {
		return types.NumericBound{}, fmt.Errorf("bound %q is not a non-negative integer", text)
	}
	return types.MakeNumericBoundUnsigned(n), nil
}

func numericBody(tag uint8, rs witTypes.Option[types.NumericRestrictions]) types.SchemaTypeBody {
	switch tag {
	case types.SchemaTypeBodyS8Type:
		return types.MakeSchemaTypeBodyS8Type(rs)
	case types.SchemaTypeBodyS16Type:
		return types.MakeSchemaTypeBodyS16Type(rs)
	case types.SchemaTypeBodyS32Type:
		return types.MakeSchemaTypeBodyS32Type(rs)
	case types.SchemaTypeBodyS64Type:
		return types.MakeSchemaTypeBodyS64Type(rs)
	case types.SchemaTypeBodyU8Type:
		return types.MakeSchemaTypeBodyU8Type(rs)
	case types.SchemaTypeBodyU16Type:
		return types.MakeSchemaTypeBodyU16Type(rs)
	case types.SchemaTypeBodyU32Type:
		return types.MakeSchemaTypeBodyU32Type(rs)
	case types.SchemaTypeBodyU64Type:
		return types.MakeSchemaTypeBodyU64Type(rs)
	case types.SchemaTypeBodyF32Type:
		return types.MakeSchemaTypeBodyF32Type(rs)
	}
	return types.MakeSchemaTypeBodyF64Type(rs)
}

var quantityBound = regexp.MustCompile(`^(-?[0-9]+)(?:\.([0-9]+))?\s*([^0-9\s].*)?$`)

// parseQuantityBound reads "1.5kg", or "1.5" in the base unit.
func parseQuantityBound(text, base string) (types.QuantityValue, error) {
	m := quantityBound.FindStringSubmatch(strings.TrimSpace(text))
	if m == nil {
		return types.QuantityValue{}, fmt.Errorf("quantity bound %q must be a decimal number and an optional unit", text)
	}
	mantissa, err := strconv.ParseInt(m[1]+m[2], 10, 64)
	if err != nil {
		return types.QuantityValue{}, fmt.Errorf("quantity bound %q is out of range", text)
	}
	unit := m[3]
	if unit == "" {
		unit = base
	}
	return types.QuantityValue{Mantissa: mantissa, Scale: int32(len(m[2])), Unit: unit}, nil
}

// boundText renders a typed bound the way a tag spells it.
func boundText(v reflect.Value) (string, error) {
	for v.Kind() == reflect.Pointer {
		if v.IsNil() {
			return "", fmt.Errorf("a bound cannot be nil")
		}
		v = v.Elem()
	}
	if elem, some, ok := values.OptionGet(v.Interface()); ok {
		if !some {
			return "", fmt.Errorf("a bound cannot be None")
		}
		return boundText(elem)
	}
	if mantissa, scale, unit, ok := values.QuantityParts(v.Interface()); ok {
		return quantityText(mantissa, scale, unit), nil
	}
	switch v.Kind() {
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return strconv.FormatInt(v.Int(), 10), nil
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return strconv.FormatUint(v.Uint(), 10), nil
	case reflect.Float32, reflect.Float64:
		return strconv.FormatFloat(v.Float(), 'g', -1, 64), nil
	}
	return "", fmt.Errorf("a %s cannot bound a range", v.Type())
}

func quantityText(mantissa int64, scale int32, unit string) string {
	digits := strconv.FormatInt(mantissa, 10)
	if scale > 0 {
		neg := strings.HasPrefix(digits, "-")
		digits = strings.TrimPrefix(digits, "-")
		for len(digits) <= int(scale) {
			digits = "0" + digits
		}
		digits = digits[:len(digits)-int(scale)] + "." + digits[len(digits)-int(scale):]
		if neg {
			digits = "-" + digits
		}
	}
	return digits + unit
}

// --- applying restrictions to a graph -----------------------------------------

// restrictedNode is c's node, or a copy of it carrying r's restrictions.
func (g *graphBuilder) restrictedNode(c *codec, r *restriction) int32 {
	idx := g.node(c)
	if r == nil {
		return idx
	}
	restricted, err := g.restrictAt(idx, r)
	if err != nil {
		if g.invalids == nil {
			g.invalids = map[reflect.Type]string{}
		}
		g.invalids[c.typ] = err.Error()
		return idx
	}
	return restricted
}

func (g *graphBuilder) restrictAt(idx int32, r *restriction) (int32, error) {
	node := g.nodes[idx]
	switch node.Body.Tag() {
	case types.SchemaTypeBodyOptionType:
		inner, err := g.restrictAt(node.Body.OptionType(), r)
		if err != nil {
			return idx, err
		}
		g.nodes = append(g.nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyOptionType(inner), Metadata: node.Metadata})
		return int32(len(g.nodes) - 1), nil
	case types.SchemaTypeBodyListType:
		element, err := g.restrictAt(node.Body.ListType(), r)
		if err != nil {
			return idx, err
		}
		g.nodes = append(g.nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyListType(element), Metadata: node.Metadata})
		return int32(len(g.nodes) - 1), nil
	case types.SchemaTypeBodyRefType:
		return idx, fmt.Errorf("restrictions cannot apply to a recursive type")
	}
	body, err := r.apply(node.Body)
	if err != nil {
		return idx, err
	}
	g.nodes = append(g.nodes, types.SchemaTypeNode{Body: body, Metadata: node.Metadata})
	return int32(len(g.nodes) - 1), nil
}

// restrictionFailed is c, marked invalid for a restriction that cannot apply:
// the definition error names the field, and the type still emits a node.
func restrictionFailed(c *codec, reason string) *codec {
	bad := *c
	bad.invalid = reason
	return &bad
}

// checkRestriction reports a restriction that cannot apply to c's type,
// without touching any real graph.
func (d *definitions) checkRestriction(c *codec, r *restriction) error {
	g := &graphBuilder{d: d}
	_, err := g.restrictAt(g.node(c), r)
	return err
}

// --- tool argument setters ------------------------------------------------------

// restrictable gives a tool argument binding its restriction setters. A
// returns the binding, for chaining; T is the argument's value type, which
// typed bounds are spelled in.
type restrictable[A any, T any] struct {
	rb   *argBinding
	self A
}

func (r restrictable[A, T]) restriction() *restriction {
	if r.rb.restrict == nil {
		r.rb.restrict = &restriction{}
	}
	return r.rb.restrict
}

func (r restrictable[A, T]) bound(dst **string, v T) {
	text, err := boundText(reflect.ValueOf(&v).Elem())
	if err != nil {
		r.rb.restrictErr = err
		return
	}
	*dst = &text
}

// Range bounds a number or quantity argument, inclusively.
func (r restrictable[A, T]) Range(min, max T) A {
	r.bound(&r.restriction().min, min)
	r.bound(&r.restriction().max, max)
	return r.self
}

// MinValue is the smallest value a number or quantity argument accepts.
func (r restrictable[A, T]) MinValue(min T) A {
	r.bound(&r.restriction().min, min)
	return r.self
}

// MaxValue is the largest value a number or quantity argument accepts.
func (r restrictable[A, T]) MaxValue(max T) A {
	r.bound(&r.restriction().max, max)
	return r.self
}

// Unit names the unit a number argument is measured in.
func (r restrictable[A, T]) Unit(unit string) A {
	r.restriction().unit = &unit
	return r.self
}

// Languages lists the BCP-47 languages a text argument may be in.
func (r restrictable[A, T]) Languages(codes ...string) A {
	r.restriction().languages = &codes
	return r.self
}

// MinLength is the fewest characters a text argument may have.
func (r restrictable[A, T]) MinLength(n uint32) A {
	r.restriction().minLength = &n
	return r.self
}

// MaxLength is the most characters a text argument may have.
func (r restrictable[A, T]) MaxLength(n uint32) A {
	r.restriction().maxLength = &n
	return r.self
}

// Regex is a pattern a text argument must match.
func (r restrictable[A, T]) Regex(pattern string) A {
	if _, err := regexp.Compile(pattern); err != nil {
		r.rb.restrictErr = fmt.Errorf("regex %q: %w", pattern, err)
	}
	r.restriction().regex = &pattern
	return r.self
}

// Mime lists the media types a binary or path argument may have.
func (r restrictable[A, T]) Mime(types ...string) A {
	r.restriction().mime = &types
	return r.self
}

// MinBytes is the smallest a binary argument may be.
func (r restrictable[A, T]) MinBytes(n uint32) A {
	r.restriction().minBytes = &n
	return r.self
}

// MaxBytes is the largest a binary argument may be.
func (r restrictable[A, T]) MaxBytes(n uint32) A {
	r.restriction().maxBytes = &n
	return r.self
}

// Direction is which way a path argument's file flows.
func (r restrictable[A, T]) Direction(d PathDirection) A {
	r.restriction().direction = &d
	return r.self
}

// PathKind is whether a path argument names a file, a directory or either.
func (r restrictable[A, T]) PathKind(k PathKind) A {
	r.restriction().kind = &k
	return r.self
}

// Extensions lists the file extensions a path argument may have.
func (r restrictable[A, T]) Extensions(exts ...string) A {
	r.restriction().extensions = &exts
	return r.self
}

// Schemes lists the schemes a URL argument may use.
func (r restrictable[A, T]) Schemes(schemes ...string) A {
	r.restriction().schemes = &schemes
	return r.self
}

// Hosts lists the hosts a URL argument may name.
func (r restrictable[A, T]) Hosts(hosts ...string) A {
	r.restriction().hosts = &hosts
	return r.self
}
