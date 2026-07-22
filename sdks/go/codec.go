// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"fmt"
	"reflect"
	"sort"

	types "github.com/golemcloud/golem-go/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// A codec is everything the SDK knows about one Go type: how to describe it in
// the schema graph, how to encode a value of it, and how to decode one.
//
// Bundling the three together is deliberate. The schema and the value tree must
// agree structurally — a record type with three fields must be matched by a
// record value with three children, in the same order — and nothing on the wire
// enforces that. Deriving all three from one place makes disagreement
// impossible by construction, instead of a convention three separate walks have
// to keep by hand.
//
// Nesting is composition: the codec for Option[Result[T, E]] is built from the
// codec for Result[T, E], which is built from the codecs for T and E. No
// composite needs to know how deep it sits.
type codec struct {
	typ    reflect.Type
	body   func(*graphBuilder) types.SchemaTypeBody
	encode func(*valBuilder, reflect.Value) int32
	decode func(*decoder, reflect.Value, int32) error
}

// codecCache memoizes compilation. Compilation happens during registration
// (package init), which is single-goroutine, so no locking is needed.
var codecCache = map[reflect.Type]*codec{}

// compile returns the codec for t, building it if this is the first request.
//
// The cache entry is installed *before* the children are compiled, so a
// self-referential type resolves to the in-progress codec rather than recursing
// forever. Its function fields are still nil at that moment, but every use is
// behind a closure that dereferences them at call time, by which point the
// outer compile has filled them in.
func compile(t reflect.Type) *codec {
	if c, ok := codecCache[t]; ok {
		return c
	}
	c := &codec{typ: t}
	codecCache[t] = c
	buildCodec(c)
	return c
}

func buildCodec(c *codec) {
	// User-declared variants and enums are looked up before the kind switch:
	// an enum is a named integer, which would otherwise compile as a plain
	// integer, and a variant is an interface, which has no other meaning.
	if d, ok := variantRegistry[c.typ]; ok {
		compileVariant(c, d)
		return
	}
	if d, ok := enumRegistry[c.typ]; ok {
		compileEnum(c, d)
		return
	}
	// SDK composite types are structs, so they must be recognised before the
	// generic struct-as-record case. The check is an interface assertion on the
	// zero value, done once here rather than per value.
	if sdkComposite(c) {
		return
	}

	none := witTypes.None[types.NumericRestrictions]()

	switch c.typ.Kind() {
	case reflect.String:
		scalar(c, types.MakeSchemaTypeBodyStringType(), types.SchemaValueNodeStringValue,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeStringValue(v.String()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetString(n.StringValue()) })

	case reflect.Bool:
		scalar(c, types.MakeSchemaTypeBodyBoolType(), types.SchemaValueNodeBoolValue,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeBoolValue(v.Bool()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetBool(n.BoolValue()) })

	case reflect.Int64:
		scalar(c, types.MakeSchemaTypeBodyS64Type(none), types.SchemaValueNodeS64Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeS64Value(v.Int()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetInt(n.S64Value()) })

	case reflect.Int32:
		scalar(c, types.MakeSchemaTypeBodyS32Type(none), types.SchemaValueNodeS32Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeS32Value(int32(v.Int())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetInt(int64(n.S32Value())) })

	case reflect.Int16:
		scalar(c, types.MakeSchemaTypeBodyS16Type(none), types.SchemaValueNodeS16Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeS16Value(int16(v.Int())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetInt(int64(n.S16Value())) })

	case reflect.Int8:
		scalar(c, types.MakeSchemaTypeBodyS8Type(none), types.SchemaValueNodeS8Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeS8Value(int8(v.Int())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetInt(int64(n.S8Value())) })

	case reflect.Uint64:
		scalar(c, types.MakeSchemaTypeBodyU64Type(none), types.SchemaValueNodeU64Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeU64Value(v.Uint()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetUint(n.U64Value()) })

	case reflect.Uint32:
		scalar(c, types.MakeSchemaTypeBodyU32Type(none), types.SchemaValueNodeU32Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeU32Value(uint32(v.Uint())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetUint(uint64(n.U32Value())) })

	case reflect.Uint16:
		scalar(c, types.MakeSchemaTypeBodyU16Type(none), types.SchemaValueNodeU16Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeU16Value(uint16(v.Uint())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetUint(uint64(n.U16Value())) })

	case reflect.Uint8:
		scalar(c, types.MakeSchemaTypeBodyU8Type(none), types.SchemaValueNodeU8Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeU8Value(uint8(v.Uint())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetUint(uint64(n.U8Value())) })

	case reflect.Float64:
		scalar(c, types.MakeSchemaTypeBodyF64Type(none), types.SchemaValueNodeF64Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeF64Value(v.Float()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetFloat(n.F64Value()) })

	case reflect.Float32:
		scalar(c, types.MakeSchemaTypeBodyF32Type(none), types.SchemaValueNodeF32Value,
			func(b *valBuilder, v reflect.Value) int32 {
				return b.push(types.MakeSchemaValueNodeF32Value(float32(v.Float())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetFloat(float64(n.F32Value())) })

	case reflect.Struct:
		compileRecord(c)

	case reflect.Pointer:
		// A pointer is Go's spelling of "optional". This is the same convention
		// encoding/json, protobuf-go and serde use, so it should not surprise —
		// but it does mean a *T used purely to avoid copying still publishes as
		// option<T> to callers.
		compileOption(c, compile(c.typ.Elem()), pointerOps(c.typ))

	case reflect.Slice:
		compileList(c, compile(c.typ.Elem()))

	case reflect.Array:
		compileFixedList(c, compile(c.typ.Elem()))

	case reflect.Map:
		compileMap(c)

	case reflect.Int, reflect.Uint:
		panic(fmt.Sprintf("golem: %s has a platform-dependent width; use a sized type such as int64/uint64", c.typ))

	case reflect.Interface:
		panic(fmt.Sprintf("golem: interface type %s is not a registered variant; declare it with golem.DefineVariant", c.typ))

	default:
		panic(fmt.Sprintf("golem: unsupported type %s (kind %s)", c.typ, c.typ.Kind()))
	}
}

// sdkComposite recognises the SDK's own composite types (Option, Result, ...)
// and fills in c if c.typ is one of them.
func sdkComposite(c *codec) bool {
	if c.typ.Kind() != reflect.Struct {
		return false
	}
	zero := reflect.New(c.typ).Elem().Interface()

	switch z := zero.(type) {
	case optionish:
		compileOption(c, compile(z.optionElem()), optionValueOps())
		return true
	case resultish:
		okT, errT := z.resultElems()
		compileResult(c, compile(okT), compile(errT))
		return true
	}
	return false
}

// scalar fills in a codec for a type that occupies a single node on both sides.
// Decoding checks the tag first: the generated accessors panic on mismatch, and
// malformed input must produce an error, not a panic.
func scalar(
	c *codec,
	body types.SchemaTypeBody,
	tag uint8,
	enc func(*valBuilder, reflect.Value) int32,
	set func(reflect.Value, types.SchemaValueNode),
) {
	c.body = func(*graphBuilder) types.SchemaTypeBody { return body }
	c.encode = enc
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != tag {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		set(dst, n)
		return nil
	}
}

// compileRecord handles Go structs, which lower to WIT records. Fields are
// positional on the wire: declaration order is the order the schema reports and
// the order values are written in.
func compileRecord(c *codec) {
	fields := structFields(c.typ)

	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		nf := make([]types.NamedFieldType, 0, len(fields))
		for _, f := range fields {
			nf = append(nf, types.NamedFieldType{Name: f.name, Body: g.node(f.codec)})
		}
		return types.MakeSchemaTypeBodyRecordType(nf)
	}

	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		// Children are appended first; the record node then refers to them.
		idxs := make([]int32, 0, len(fields))
		for _, f := range fields {
			idxs = append(idxs, f.codec.encode(b, v.Field(f.index)))
		}
		return b.push(types.MakeSchemaValueNodeRecordValue(idxs))
	}

	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeRecordValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		idxs := n.RecordValue()
		if len(idxs) < len(fields) {
			return fmt.Errorf("record for %s has %d field(s), want %d", c.typ, len(idxs), len(fields))
		}
		for i, f := range fields {
			if err := f.codec.decode(d, dst.Field(f.index), idxs[i]); err != nil {
				return fmt.Errorf("%s.%s: %w", c.typ, f.name, err)
			}
		}
		return nil
	}
}

// ---------------------------------------------------------------------------
// containers
// ---------------------------------------------------------------------------

// optionOps abstracts the two Go spellings of an optional value, so *T and
// Option[T] share one codec and therefore produce identical schemas.
type optionOps struct {
	// get reports whether the value is present, and if so yields the inner value.
	get func(v reflect.Value) (reflect.Value, bool)
	// setNone clears dst.
	setNone func(dst reflect.Value)
	// setSome prepares dst to hold a value and returns the slot to decode into.
	setSome func(dst reflect.Value) reflect.Value
}

func pointerOps(t reflect.Type) optionOps {
	return optionOps{
		get: func(v reflect.Value) (reflect.Value, bool) {
			if v.IsNil() {
				return reflect.Value{}, false
			}
			return v.Elem(), true
		},
		setNone: func(dst reflect.Value) { dst.Set(reflect.Zero(t)) },
		setSome: func(dst reflect.Value) reflect.Value {
			p := reflect.New(t.Elem())
			dst.Set(p)
			return p.Elem()
		},
	}
}

func compileOption(c *codec, inner *codec, ops optionOps) {
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyOptionType(g.node(inner))
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		if in, some := ops.get(v); some {
			idx := inner.encode(b, in)
			return b.push(types.MakeSchemaValueNodeOptionValue(witTypes.Some(idx)))
		}
		return b.push(types.MakeSchemaValueNodeOptionValue(witTypes.None[int32]()))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeOptionValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		opt := n.OptionValue()
		if opt.IsNone() {
			ops.setNone(dst)
			return nil
		}
		return inner.decode(d, ops.setSome(dst), opt.Some())
	}
}

func compileList(c *codec, elem *codec) {
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyListType(g.node(elem))
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		idxs := make([]int32, 0, v.Len())
		for i := range v.Len() {
			idxs = append(idxs, elem.encode(b, v.Index(i)))
		}
		return b.push(types.MakeSchemaValueNodeListValue(idxs))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeListValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		idxs := n.ListValue()
		out := reflect.MakeSlice(c.typ, len(idxs), len(idxs))
		for i, child := range idxs {
			if err := elem.decode(d, out.Index(i), child); err != nil {
				return fmt.Errorf("%s[%d]: %w", c.typ, i, err)
			}
		}
		dst.Set(out)
		return nil
	}
}

// compileFixedList handles Go arrays, whose length is part of the type.
func compileFixedList(c *codec, elem *codec) {
	n := c.typ.Len()
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyFixedListType(types.FixedListSpec{
			Element: g.node(elem),
			Length:  uint32(n),
		})
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		idxs := make([]int32, 0, n)
		for i := range n {
			idxs = append(idxs, elem.encode(b, v.Index(i)))
		}
		return b.push(types.MakeSchemaValueNodeFixedListValue(idxs))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		node, err := d.node(idx)
		if err != nil {
			return err
		}
		if node.Tag() != types.SchemaValueNodeFixedListValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", node.Tag(), c.typ)
		}
		idxs := node.FixedListValue()
		if len(idxs) != n {
			return fmt.Errorf("fixed list for %s has %d element(s), want %d", c.typ, len(idxs), n)
		}
		for i, child := range idxs {
			if err := elem.decode(d, dst.Index(i), child); err != nil {
				return fmt.Errorf("%s[%d]: %w", c.typ, i, err)
			}
		}
		return nil
	}
}

func compileMap(c *codec) {
	kt, vt := c.typ.Key(), c.typ.Elem()
	if !isPrimitiveKind(kt.Kind()) {
		panic(fmt.Sprintf("golem: map key type %s is not a primitive; WIT restricts map keys to primitives", kt))
	}
	key, val := compile(kt), compile(vt)

	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyMapType(types.MapSpec{Key: g.node(key), Value: g.node(val)})
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		// Go randomizes map iteration order. Encoding must be deterministic:
		// these trees are recorded in the oplog and compared on replay.
		keys := v.MapKeys()
		sortValues(keys)
		entries := make([]types.MapEntry, 0, len(keys))
		for _, k := range keys {
			ki := key.encode(b, k)
			vi := val.encode(b, v.MapIndex(k))
			entries = append(entries, types.MapEntry{Key: ki, Value: vi})
		}
		return b.push(types.MakeSchemaValueNodeMapValue(entries))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeMapValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		out := reflect.MakeMap(c.typ)
		for i, e := range n.MapValue() {
			k := reflect.New(kt).Elem()
			if err := key.decode(d, k, e.Key); err != nil {
				return fmt.Errorf("%s key %d: %w", c.typ, i, err)
			}
			v := reflect.New(vt).Elem()
			if err := val.decode(d, v, e.Value); err != nil {
				return fmt.Errorf("%s value %d: %w", c.typ, i, err)
			}
			out.SetMapIndex(k, v)
		}
		dst.Set(out)
		return nil
	}
}

func isPrimitiveKind(k reflect.Kind) bool {
	switch k {
	case reflect.String, reflect.Bool,
		reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64,
		reflect.Float32, reflect.Float64:
		return true
	}
	return false
}

// sortValues orders primitive map keys so encoding is deterministic.
func sortValues(vs []reflect.Value) {
	sort.Slice(vs, func(i, j int) bool {
		a, b := vs[i], vs[j]
		switch a.Kind() {
		case reflect.String:
			return a.String() < b.String()
		case reflect.Bool:
			return !a.Bool() && b.Bool()
		case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
			return a.Int() < b.Int()
		case reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
			return a.Uint() < b.Uint()
		case reflect.Float32, reflect.Float64:
			return a.Float() < b.Float()
		}
		return false
	})
}

// optionValueOps drives Option[T] through the same codec as *T, so the two
// spellings produce identical schemas and are interchangeable.
func optionValueOps() optionOps {
	return optionOps{
		get: func(v reflect.Value) (reflect.Value, bool) {
			return v.Interface().(optionish).optionGet()
		},
		setNone: func(dst reflect.Value) {
			dst.Addr().Interface().(optionSetter).optionSetNone()
		},
		setSome: func(dst reflect.Value) reflect.Value {
			return dst.Addr().Interface().(optionSetter).optionSetSome()
		},
	}
}

func compileResult(c *codec, okC, errC *codec) {
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyResultType(types.ResultSpec{
			Ok:  witTypes.Some(g.node(okC)),
			Err: witTypes.Some(g.node(errC)),
		})
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		inner, isErr := v.Interface().(resultish).resultGet()
		if isErr {
			i := errC.encode(b, inner)
			return b.push(types.MakeSchemaValueNodeResultValue(
				types.MakeResultValuePayloadErrValue(witTypes.Some(i))))
		}
		i := okC.encode(b, inner)
		return b.push(types.MakeSchemaValueNodeResultValue(
			types.MakeResultValuePayloadOkValue(witTypes.Some(i))))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeResultValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		setter := dst.Addr().Interface().(resultSetter)
		payload := n.ResultValue()
		if payload.Tag() == types.ResultValuePayloadErrValue {
			child := payload.ErrValue()
			if child.IsNone() {
				return fmt.Errorf("%s: err arm carries no value", c.typ)
			}
			return errC.decode(d, setter.resultSetErr(), child.Some())
		}
		child := payload.OkValue()
		if child.IsNone() {
			return fmt.Errorf("%s: ok arm carries no value", c.typ)
		}
		return okC.decode(d, setter.resultSetOk(), child.Some())
	}
}

// ---------------------------------------------------------------------------
// variants and enums
// ---------------------------------------------------------------------------

func compileVariant(c *codec, d *variantDef) {
	caseCodecs := make([]*codec, len(d.cases))
	byType := make(map[reflect.Type]int, len(d.cases))
	for i, cs := range d.cases {
		caseCodecs[i] = compile(cs.typ)
		byType[cs.typ] = i
	}

	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		out := make([]types.VariantCaseType, 0, len(d.cases))
		for i, cs := range d.cases {
			out = append(out, types.VariantCaseType{
				Name:    cs.name,
				Payload: witTypes.Some(g.node(caseCodecs[i])),
			})
		}
		return types.MakeSchemaTypeBodyVariantType(out)
	}

	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		concrete := v
		if v.Kind() == reflect.Interface {
			if v.IsNil() {
				panic(fmt.Sprintf("golem: nil %s cannot be encoded; a variant must hold one of its cases", c.typ))
			}
			concrete = v.Elem()
		}
		i, ok := byType[concrete.Type()]
		if !ok {
			panic(fmt.Sprintf("golem: %s is not a registered case of variant %s", concrete.Type(), c.typ))
		}
		payload := caseCodecs[i].encode(b, concrete)
		return b.push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
			Case:    uint32(i),
			Payload: witTypes.Some(payload),
		}))
	}

	c.decode = func(dec *decoder, dst reflect.Value, idx int32) error {
		n, err := dec.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeVariantValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		p := n.VariantValue()
		if int(p.Case) >= len(d.cases) {
			return fmt.Errorf("%s: case index %d out of range (%d cases)", c.typ, p.Case, len(d.cases))
		}
		if p.Payload.IsNone() {
			return fmt.Errorf("%s: case %q carries no payload", c.typ, d.cases[p.Case].name)
		}
		out := reflect.New(d.cases[p.Case].typ).Elem()
		if err := caseCodecs[p.Case].decode(dec, out, p.Payload.Some()); err != nil {
			return fmt.Errorf("%s case %q: %w", c.typ, d.cases[p.Case].name, err)
		}
		dst.Set(out)
		return nil
	}
}

func compileEnum(c *codec, d *enumDef) {
	signed := false
	switch c.typ.Kind() {
	case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		signed = true
	}

	c.body = func(*graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyEnumType(d.names)
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		var i int64
		if signed {
			i = v.Int()
		} else {
			i = int64(v.Uint())
		}
		if i < 0 || int(i) >= len(d.names) {
			panic(fmt.Sprintf("golem: %s value %d is outside the declared enum range 0..%d",
				c.typ, i, len(d.names)-1))
		}
		return b.push(types.MakeSchemaValueNodeEnumValue(uint32(i)))
	}
	c.decode = func(dec *decoder, dst reflect.Value, idx int32) error {
		n, err := dec.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeEnumValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		i := n.EnumValue()
		if int(i) >= len(d.names) {
			return fmt.Errorf("%s: enum index %d out of range (%d names)", c.typ, i, len(d.names))
		}
		if signed {
			dst.SetInt(int64(i))
		} else {
			dst.SetUint(uint64(i))
		}
		return nil
	}
}
