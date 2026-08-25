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
	"reflect"
	"sort"
	"strings"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// codec is everything the SDK knows about one Go type: how to describe it in
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
	typ  reflect.Type
	body func(*graphBuilder) types.SchemaTypeBody

	// recursive marks a type reachable from itself. WIT cannot express a raw
	// cycle in the flat node list — a consumer decoding one rejects it with
	// CyclicTypeWithoutRef — so such a type is emitted as a named def and
	// referenced through ref-type, which is the only valid recursion form.
	recursive bool
	// building is true while this codec's children are being compiled. Re-entry
	// while building is exactly what identifies a recursive type.
	building bool
	// invalid is non-empty when the type cannot be represented (unsupported
	// kind, platform-dependent width, unregistered variant, …). Set by
	// markInvalid instead of panicking; the schema builder collects these per
	// agent so the problem is attributed and reported at discovery.
	invalid string

	encode func(*valBuilder, reflect.Value) int32
	decode func(*decoder, reflect.Value, int32) error
}

// compile returns the codec for t, building it if this is the first request.
// Results are memoized in defs.codecs (compilation runs single-goroutine at
// registration, so no locking is needed).
//
// The cache entry is installed *before* the children are compiled, so a
// self-referential type resolves to the in-progress codec rather than recursing
// forever. Its function fields are still nil at that moment, but every use is
// behind a closure that dereferences them at call time, by which point the
// outer compile has filled them in.
func (d *definitions) compile(t reflect.Type) *codec {
	if c, ok := d.codecs[t]; ok {
		if c.building {
			// Reached t again while still compiling it: t is reachable from
			// itself. Marking one member of each cycle is enough — its ref-type
			// node breaks the cycle for every path through it.
			c.recursive = true
		}
		return c
	}
	c := &codec{typ: t, building: true}
	d.codecs[t] = c
	d.buildCodec(c)
	c.building = false
	return c
}

// typeID derives the stable, language-independent identifier a named def
// carries. A pinned id (via [NameType]) wins, so cross-language consumers can be
// made to agree; otherwise it is derived from the Go type's package path + name.
func (d *definitions) typeID(t reflect.Type) string {
	if id, ok := d.pins[t]; ok {
		return id
	}
	name := t.Name()
	if name == "" {
		return ""
	}
	if pkg := t.PkgPath(); pkg != "" {
		return strings.ReplaceAll(pkg, "/", ".") + "." + name
	}
	return name
}

func (d *definitions) buildCodec(c *codec) {
	// User-declared variants and enums are looked up before the kind switch:
	// an enum is a named integer, which would otherwise compile as a plain
	// integer, and a variant is an interface, which has no other meaning.
	if vd, ok := d.variants[c.typ]; ok {
		d.compileVariant(c, vd)
		return
	}
	if ed, ok := d.enums[c.typ]; ok {
		compileEnum(c, ed)
		return
	}
	// SDK composite types are structs, so they must be recognised before the
	// generic struct-as-record case. The check is an interface assertion on the
	// zero value, done once here rather than per value.
	if d.sdkComposite(c) {
		return
	}

	// Concrete named types are matched exactly, before the kind switch would
	// treat them as their underlying primitive.
	if fill, ok := namedTypeCodecs[c.typ]; ok {
		fill(c)
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
		d.compileRecord(c)

	case reflect.Pointer:
		// A pointer is Go's spelling of "optional". This is the same convention
		// encoding/json, protobuf-go and serde use, so it should not surprise —
		// but it does mean a *T used purely to avoid copying still publishes as
		// option<T> to callers.
		compileOption(c, d.compile(c.typ.Elem()), pointerOps(c.typ))

	case reflect.Slice:
		compileList(c, d.compile(c.typ.Elem()))

	case reflect.Array:
		compileFixedList(c, d.compile(c.typ.Elem()))

	case reflect.Map:
		d.compileMap(c)

	case reflect.Int, reflect.Uint:
		markInvalid(c, "%s has a platform-dependent width; use a sized type such as int64/uint64", c.typ)

	case reflect.Interface:
		markInvalid(c, "interface type %s is not a registered variant; declare it with golem.DefineVariant", c.typ)

	default:
		markInvalid(c, "unsupported type %s (kind %s)", c.typ, c.typ.Kind())
	}
}

// markInvalid flags a type the SDK cannot represent and fills the codec with a
// safe no-op body, so schema derivation itself does not panic. It records
// nothing globally: the schema builder collects invalid codecs per agent
// ([graphBuilder.invalids]) so the problem is attributed to the agent(s) that
// actually use the type and reported at discovery — never as an init() trap, and
// never poisoning an unrelated agent.
func markInvalid(c *codec, format string, args ...any) {
	c.invalid = fmt.Sprintf(format, args...)
	c.body = func(*graphBuilder) types.SchemaTypeBody { return types.MakeSchemaTypeBodyBoolType() }
	c.encode = func(b *valBuilder, _ reflect.Value) int32 {
		return b.push(types.MakeSchemaValueNodeBoolValue(false))
	}
	c.decode = func(*decoder, reflect.Value, int32) error {
		return fmt.Errorf("type %s could not be compiled (see agent definition errors)", c.typ)
	}
}

// sdkComposite recognises the SDK's own composite types (Option, Result, ...)
// and fills in c if c.typ is one of them.
func (d *definitions) sdkComposite(c *codec) bool {
	if c.typ.Kind() != reflect.Struct {
		return false
	}
	zero := reflect.New(c.typ).Elem().Interface()

	switch z := zero.(type) {
	case secretish:
		compileSecret(c, d.compile(z.secretElem()))
		return true
	case optionish:
		compileOption(c, d.compile(z.optionElem()), optionValueOps())
		return true
	case resultish:
		okT, errT := z.resultElems()
		compileResult(c, d.compile(okT), d.compile(errT))
		return true
	}
	return false
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
