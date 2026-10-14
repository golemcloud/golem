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

package engine

import (
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Case is one case of a variant: its wire name and its payload type.
type Case struct {
	Name string
	Typ  reflect.Type
	// Wrapped means Typ is a one-field struct whose field is the payload.
	Wrapped bool
}

// VariantDef is the registered case list for one interface type. Case order is
// declaration order, and it is the wire order.
type VariantDef struct {
	Iface reflect.Type
	Cases []Case
}

// wrappedPayloadErr reports why t cannot wrap a payload, or "" when it can. A
// wrapper is a struct with exactly one exported field: anything else leaves it
// unclear which part is the payload.
func wrappedPayloadErr(t reflect.Type) string {
	if t.Kind() != reflect.Struct {
		return fmt.Sprintf("%s is not a struct", t)
	}
	if t.NumField() != 1 {
		return fmt.Sprintf("%s has %d fields; a wrapper has exactly one", t, t.NumField())
	}
	if !t.Field(0).IsExported() {
		return fmt.Sprintf("%s's field %s is unexported", t, t.Field(0).Name)
	}
	return ""
}

// DefineVariant registers the closed set of types inhabiting the interface it.
func (d *Engine) DefineVariant(it reflect.Type, cases []Case) *VariantDef {
	vd := &VariantDef{Iface: it, Cases: cases}
	if it.Kind() != reflect.Interface {
		d.RecordErr("", "", "DefineVariant requires an interface type, got %s", it)
		return vd
	}
	if _, dup := d.Variants[it]; dup {
		d.RecordErr("", "", "variant already defined for %s", it)
		return vd
	}
	if len(cases) == 0 {
		d.RecordErr("", "", "DefineVariant[%s] needs at least one case", it)
	}

	seen := map[string]bool{}
	seenType := map[reflect.Type]bool{}
	for _, c := range cases {
		if !c.Typ.Implements(it) && !reflect.PointerTo(c.Typ).Implements(it) {
			d.RecordErr("", "", "variant case %s (%s) does not implement %s", c.Name, c.Typ, it)
		}
		if seen[c.Name] {
			d.RecordErr("", "", "variant %s has duplicate case name %q", it, c.Name)
		}
		if c.Wrapped {
			if reason := wrappedPayloadErr(c.Typ); reason != "" {
				d.RecordErr("", "", "variant %s: wrapped case %q: %s", it, c.Name, reason)
			}
		}
		if seenType[c.Typ] {
			// The wire case is chosen by the value's dynamic type, so one type
			// cannot map to two case names unambiguously.
			d.RecordErr("", "", "variant %s uses case type %s more than once", it, c.Typ)
		}
		seen[c.Name] = true
		seenType[c.Typ] = true
	}

	// Registered even with soft errors above, so downstream codec compilation
	// resolves the interface as a variant instead of cascading a second error.
	d.Variants[it] = vd
	return vd
}

// EnumDef is the registered name list for one named integer type. A value's
// wire representation is its position in this list.
type EnumDef struct {
	Typ   reflect.Type
	Names []string
}

// DefineEnum registers a named integer type as an enum.
func (d *Engine) DefineEnum(t reflect.Type, names []string) *EnumDef {
	ed := &EnumDef{Typ: t, Names: names}
	switch t.Kind() {
	case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
	default:
		d.RecordErr("", "", "DefineEnum requires a named integer type, got %s (kind %s)", t, t.Kind())
		return ed
	}
	if _, dup := d.Enums[t]; dup {
		d.RecordErr("", "", "enum already defined for %s", t)
		return ed
	}
	if len(names) == 0 {
		d.RecordErr("", "", "DefineEnum[%s] needs at least one name", t)
	}
	d.Enums[t] = ed
	return ed
}

// FlagsDef is the registered flag set for one struct type.
type FlagsDef struct {
	Typ    reflect.Type
	Names  []string
	Fields []int // struct field index per name, parallel to Names
}

// DefineFlags registers a struct of bool fields as a flags set.
func (d *Engine) DefineFlags(t reflect.Type) *FlagsDef {
	fd := &FlagsDef{Typ: t}
	if t.Kind() != reflect.Struct {
		d.RecordErr("", "", "DefineFlags requires a struct type, got %s (kind %s)", t, t.Kind())
		return fd
	}
	if _, dup := d.Flags[t]; dup {
		d.RecordErr("", "", "flags already defined for %s", t)
		return fd
	}
	for i := range t.NumField() {
		f := t.Field(i)
		if f.PkgPath != "" { // unexported
			continue
		}
		if f.Type.Kind() != reflect.Bool {
			d.RecordErr("", "", "DefineFlags[%s]: field %s is %s, but every flag field must be bool",
				t, f.Name, f.Type)
			return fd
		}
		fd.Names = append(fd.Names, SchemaName(f.Name))
		fd.Fields = append(fd.Fields, i)
	}
	if len(fd.Names) == 0 {
		d.RecordErr("", "", "DefineFlags[%s] needs at least one exported bool field", t)
	}
	d.Flags[t] = fd
	return fd
}

// compileFlags lowers a registered flag struct to the WIT flags type: the type
// node carries the names, the value node one bool per name in the same order.
func compileFlags(c *Codec, fd *FlagsDef) {
	names := append([]string(nil), fd.Names...)
	fields := fd.Fields
	c.Body = func(*GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyFlagsType(append([]string(nil), names...))
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		set := make([]bool, len(fields))
		for i, fi := range fields {
			set[i] = v.Field(fi).Bool()
		}
		return b.Push(types.MakeSchemaValueNodeFlagsValue(set))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeFlagsValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		set := n.FlagsValue()
		if len(set) != len(fields) {
			return fmt.Errorf("%s: flags value has %d entries, but %s declares %d",
				c.Typ, len(set), c.Typ, len(fields))
		}
		for i, fi := range fields {
			dst.Field(fi).SetBool(set[i])
		}
		return nil
	}
}

// Branch is one branch of a union: its tag, body type and discriminator rule.
type Branch struct {
	Tag  string
	Typ  reflect.Type
	Rule types.DiscriminatorRule
	// Wrapped means Typ is a one-field struct whose field is the body.
	Wrapped bool
}

// UnionDef is the registered branch list for one interface type. Branch order
// is declaration order and is the order a decoder tries them.
type UnionDef struct {
	Iface    reflect.Type
	Branches []Branch
}

// DefineUnion registers the closed set of types inhabiting the interface it as
// an inferred-tag union.
func (d *Engine) DefineUnion(it reflect.Type, branches []Branch) *UnionDef {
	ud := &UnionDef{Iface: it, Branches: branches}
	if it.Kind() != reflect.Interface {
		d.RecordErr("", "", "DefineUnion requires an interface type, got %s", it)
		return ud
	}
	if _, dup := d.Unions[it]; dup {
		d.RecordErr("", "", "union already defined for %s", it)
		return ud
	}
	if _, dup := d.Variants[it]; dup {
		d.RecordErr("", "", "%s is already defined as a variant; it cannot also be a union", it)
		return ud
	}
	if len(branches) == 0 {
		d.RecordErr("", "", "DefineUnion[%s] needs at least one branch", it)
	}
	seen := map[string]bool{}
	for _, b := range branches {
		if seen[b.Tag] {
			d.RecordErr("", "", "DefineUnion[%s]: duplicate branch tag %q", it, b.Tag)
		}
		seen[b.Tag] = true
		if b.Wrapped {
			if reason := wrappedPayloadErr(b.Typ); reason != "" {
				d.RecordErr("", "", "DefineUnion[%s]: wrapped branch %q: %s", it, b.Tag, reason)
			}
		}
		if !b.Typ.Implements(it) {
			d.RecordErr("", "", "DefineUnion[%s]: branch %q has type %s, which does not implement %s",
				it, b.Tag, b.Typ, it)
		}
	}
	d.Unions[it] = ud
	return ud
}

// compileUnion lowers a registered union. A value encodes as its branch body
// plus the resolved tag, which the receiver uses instead of re-running the
// discriminator rules.
func (d *Engine) compileUnion(c *Codec, ud *UnionDef) {
	branchCodecs := make([]*Codec, len(ud.Branches))
	byType := make(map[reflect.Type]int, len(ud.Branches))
	for i, b := range ud.Branches {
		branchCodecs[i] = d.Compile(payloadType(b.Typ, b.Wrapped))
		byType[b.Typ] = i
	}

	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		out := make([]types.UnionBranch, 0, len(ud.Branches))
		for i, b := range ud.Branches {
			out = append(out, types.UnionBranch{
				Tag:           b.Tag,
				Body:          g.Node(branchCodecs[i]),
				Discriminator: b.Rule,
				Metadata:      types.MetadataEnvelope{},
			})
		}
		return types.MakeSchemaTypeBodyUnionType(types.UnionSpec{Branches: out})
	}

	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		concrete := v
		if v.Kind() == reflect.Interface {
			if v.IsNil() {
				panic(&EncodeError{fmt.Sprintf("nil %s cannot be encoded; a union must hold one of its branches", c.Typ)})
			}
			concrete = v.Elem()
		}
		i, ok := byType[concrete.Type()]
		if !ok {
			panic(&EncodeError{fmt.Sprintf("%s is not a registered branch of union %s", concrete.Type(), c.Typ)})
		}
		body := branchCodecs[i].Encode(b, payloadValue(concrete, ud.Branches[i].Wrapped))
		return b.Push(types.MakeSchemaValueNodeUnionValue(types.UnionValuePayload{
			Tag:  ud.Branches[i].Tag,
			Body: body,
		}))
	}

	c.Decode = func(dec *Decoder, dst reflect.Value, idx int32) error {
		n, err := dec.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeUnionValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		p := n.UnionValue()
		for i, b := range ud.Branches {
			if b.Tag != p.Tag {
				continue
			}
			out := reflect.New(b.Typ).Elem()
			if err := branchCodecs[i].Decode(dec, payloadValue(out, b.Wrapped), p.Body); err != nil {
				return fmt.Errorf("%s branch %q: %w", c.Typ, b.Tag, err)
			}
			dst.Set(out)
			return nil
		}
		return fmt.Errorf("%s: unknown union branch tag %q", c.Typ, p.Tag)
	}
}

// PinTypeID pins t's language-independent type-id.
func (d *Engine) PinTypeID(t reflect.Type, id string) {
	if id == "" {
		d.RecordErr("", "", "NameType[%s] requires a non-empty type-id", t)
		return
	}
	if existing, dup := d.Pins[t]; dup && existing != id {
		d.RecordErr("", "", "type %s already pinned to type-id %q", t, existing)
		return
	}
	for other, existing := range d.Pins {
		if existing == id && other != t {
			d.RecordErr("", "", "type-id %q already pinned to %s", id, other)
			return
		}
	}
	d.Pins[t] = id
}
