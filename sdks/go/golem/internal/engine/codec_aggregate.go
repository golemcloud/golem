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
	"encoding/binary"
	"fmt"
	"github.com/golemcloud/golem/sdks/go/core/values"
	"reflect"
	"slices"
	"time"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// compileRecord handles Go structs, which lower to WIT records. Fields are
// positional on the wire: declaration order is the order the schema reports and
// the order values are written in.
func (d *Engine) compileRecord(c *Codec) {
	fields := d.StructFields(c.Typ)
	if slices.ContainsFunc(fields, func(f Field) bool { return f.AutoInjected }) {
		MarkInvalid(c, "%s", MisplacedPrincipal)
		return
	}

	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		nf := make([]types.NamedFieldType, 0, len(fields))
		for _, f := range fields {
			nf = append(nf, types.NamedFieldType{Name: f.Name, Body: g.RestrictedNode(f.Codec, f.Restrict)})
		}
		return types.MakeSchemaTypeBodyRecordType(nf)
	}

	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		// Children are appended first; the record node then refers to them.
		idxs := make([]int32, 0, len(fields))
		for _, f := range fields {
			idxs = append(idxs, f.Codec.Encode(b, v.Field(f.Index)))
		}
		return b.Push(types.MakeSchemaValueNodeRecordValue(idxs))
	}

	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeRecordValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		idxs := n.RecordValue()
		if len(idxs) < len(fields) {
			return fmt.Errorf("record for %s has %d field(s), want %d", c.Typ, len(idxs), len(fields))
		}
		for i, f := range fields {
			if err := f.Codec.Decode(d, dst.Field(f.Index), idxs[i]); err != nil {
				return fmt.Errorf("%s.%s: %w", c.Typ, f.Name, err)
			}
		}
		return nil
	}
}

// ---------------------------------------------------------------------------
// containers
// ---------------------------------------------------------------------------

func compileList(c *Codec, elem *Codec) {
	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyListType(g.Node(elem))
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		idxs := make([]int32, 0, v.Len())
		for i := range v.Len() {
			idxs = append(idxs, elem.Encode(b, v.Index(i)))
		}
		return b.Push(types.MakeSchemaValueNodeListValue(idxs))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeListValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		idxs := n.ListValue()
		out := reflect.MakeSlice(c.Typ, len(idxs), len(idxs))
		for i, child := range idxs {
			if err := elem.Decode(d, out.Index(i), child); err != nil {
				return fmt.Errorf("%s[%d]: %w", c.Typ, i, err)
			}
		}
		dst.Set(out)
		return nil
	}
}

// compileFixedList handles Go arrays, whose length is part of the type.
func compileFixedList(c *Codec, elem *Codec) {
	n := c.Typ.Len()
	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyFixedListType(types.FixedListSpec{
			Element: g.Node(elem),
			Length:  uint32(n),
		})
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		idxs := make([]int32, 0, n)
		for i := range n {
			idxs = append(idxs, elem.Encode(b, v.Index(i)))
		}
		return b.Push(types.MakeSchemaValueNodeFixedListValue(idxs))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		node, err := d.Node(idx)
		if err != nil {
			return err
		}
		if node.Tag() != types.SchemaValueNodeFixedListValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", node.Tag(), c.Typ)
		}
		idxs := node.FixedListValue()
		if len(idxs) != n {
			return fmt.Errorf("fixed list for %s has %d element(s), want %d", c.Typ, len(idxs), n)
		}
		for i, child := range idxs {
			if err := elem.Decode(d, dst.Index(i), child); err != nil {
				return fmt.Errorf("%s[%d]: %w", c.Typ, i, err)
			}
		}
		return nil
	}
}

func (d *Engine) compileMap(c *Codec) {
	kt, vt := c.Typ.Key(), c.Typ.Elem()
	if !isPrimitiveKind(kt.Kind()) {
		MarkInvalid(c, "map key type %s is not a primitive; WIT restricts map keys to primitives", kt)
		return
	}
	key, val := d.Compile(kt), d.Compile(vt)

	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyMapType(types.MapSpec{Key: g.Node(key), Value: g.Node(val)})
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		// Go randomizes map iteration order. Encoding must be deterministic:
		// these trees are recorded in the oplog and compared on replay.
		keys := v.MapKeys()
		sortValues(keys)
		entries := make([]types.MapEntry, 0, len(keys))
		for _, k := range keys {
			ki := key.Encode(b, k)
			vi := val.Encode(b, v.MapIndex(k))
			entries = append(entries, types.MapEntry{Key: ki, Value: vi})
		}
		return b.Push(types.MakeSchemaValueNodeMapValue(entries))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeMapValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		out := reflect.MakeMap(c.Typ)
		for i, e := range n.MapValue() {
			k := reflect.New(kt).Elem()
			if err := key.Decode(d, k, e.Key); err != nil {
				return fmt.Errorf("%s key %d: %w", c.Typ, i, err)
			}
			v := reflect.New(vt).Elem()
			if err := val.Decode(d, v, e.Value); err != nil {
				return fmt.Errorf("%s value %d: %w", c.Typ, i, err)
			}
			out.SetMapIndex(k, v)
		}
		dst.Set(out)
		return nil
	}
}

func compileResult(c *Codec, okC, errC *Codec) {
	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyResultType(types.ResultSpec{
			Ok:  witTypes.Some(g.Node(okC)),
			Err: witTypes.Some(g.Node(errC)),
		})
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		inner, isErr, _ := values.ResultGet(v.Interface())
		if isErr {
			i := errC.Encode(b, inner)
			return b.Push(types.MakeSchemaValueNodeResultValue(
				types.MakeResultValuePayloadErrValue(witTypes.Some(i))))
		}
		i := okC.Encode(b, inner)
		return b.Push(types.MakeSchemaValueNodeResultValue(
			types.MakeResultValuePayloadOkValue(witTypes.Some(i))))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeResultValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		setter := dst.Addr().Interface()
		payload := n.ResultValue()
		if payload.Tag() == types.ResultValuePayloadErrValue {
			child := payload.ErrValue()
			if child.IsNone() {
				return fmt.Errorf("%s: err arm carries no value", c.Typ)
			}
			elem, _ := values.ResultSetErr(setter)
			return errC.Decode(d, elem, child.Some())
		}
		child := payload.OkValue()
		if child.IsNone() {
			return fmt.Errorf("%s: ok arm carries no value", c.Typ)
		}
		elem, _ := values.ResultSetOk(setter)
		return okC.Decode(d, elem, child.Some())
	}
}

// ---------------------------------------------------------------------------
// variants and enums
// ---------------------------------------------------------------------------

func (d *Engine) compileVariant(c *Codec, vd *VariantDef) {
	caseCodecs := make([]*Codec, len(vd.Cases))
	unit := make([]bool, len(vd.Cases))
	byType := make(map[reflect.Type]int, len(vd.Cases))
	for i, cs := range vd.Cases {
		unit[i] = !cs.Wrapped && isUnitCase(cs.Typ)
		if !unit[i] {
			caseCodecs[i] = d.Compile(payloadType(cs.Typ, cs.Wrapped))
		}
		byType[cs.Typ] = i
	}

	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		out := make([]types.VariantCaseType, 0, len(vd.Cases))
		for i, cs := range vd.Cases {
			payload := witTypes.None[int32]()
			if !unit[i] {
				payload = witTypes.Some(g.Node(caseCodecs[i]))
			}
			out = append(out, types.VariantCaseType{Name: cs.Name, Payload: payload})
		}
		return types.MakeSchemaTypeBodyVariantType(out)
	}

	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		concrete := v
		if v.Kind() == reflect.Interface {
			if v.IsNil() {
				panic(&EncodeError{fmt.Sprintf("nil %s cannot be encoded; a variant must hold one of its cases", c.Typ)})
			}
			concrete = v.Elem()
		}
		i, ok := byType[concrete.Type()]
		if !ok {
			panic(&EncodeError{fmt.Sprintf("%s is not a registered case of variant %s", concrete.Type(), c.Typ)})
		}
		if unit[i] {
			return b.Push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
				Case:    uint32(i),
				Payload: witTypes.None[int32](),
			}))
		}
		payload := caseCodecs[i].Encode(b, payloadValue(concrete, vd.Cases[i].Wrapped))
		return b.Push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
			Case:    uint32(i),
			Payload: witTypes.Some(payload),
		}))
	}

	c.Decode = func(dec *Decoder, dst reflect.Value, idx int32) error {
		n, err := dec.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeVariantValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		p := n.VariantValue()
		if int(p.Case) >= len(vd.Cases) {
			return fmt.Errorf("%s: case index %d out of range (%d cases)", c.Typ, p.Case, len(vd.Cases))
		}
		out := reflect.New(vd.Cases[p.Case].Typ).Elem()
		if unit[p.Case] {
			if p.Payload.IsSome() {
				return fmt.Errorf("%s: case %q declares no payload but one arrived", c.Typ, vd.Cases[p.Case].Name)
			}
			dst.Set(out)
			return nil
		}
		if p.Payload.IsNone() {
			return fmt.Errorf("%s: case %q carries no payload", c.Typ, vd.Cases[p.Case].Name)
		}
		if err := caseCodecs[p.Case].Decode(dec, payloadValue(out, vd.Cases[p.Case].Wrapped), p.Payload.Some()); err != nil {
			return fmt.Errorf("%s case %q: %w", c.Typ, vd.Cases[p.Case].Name, err)
		}
		dst.Set(out)
		return nil
	}
}

// payloadType is the type a case or branch publishes: the type itself, or for a
// wrapper, its single field's type.
func payloadType(t reflect.Type, wrapped bool) reflect.Type {
	if wrapped {
		return t.Field(0).Type
	}
	return t
}

// payloadValue is the part of a case or branch value that travels: the value
// itself, or for a wrapper, its single field. It stays settable when v is, so
// decoding writes straight into the wrapper.
func payloadValue(v reflect.Value, wrapped bool) reflect.Value {
	if wrapped {
		return v.Field(0)
	}
	return v
}

// isUnitCase reports whether a variant case carries no payload. Go spells a
// case with nothing in it as an empty struct — `type Cash struct{}` — so that
// is what "no payload" means here. Publishing it as an empty record instead
// would be a different schema from a payloadless case, and a Go caller could
// then neither send nor receive the payloadless cases another language
// declares.
func isUnitCase(t reflect.Type) bool {
	return t.Kind() == reflect.Struct && t.NumField() == 0
}

func compileEnum(c *Codec, d *EnumDef) {
	signed := false
	switch c.Typ.Kind() {
	case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		signed = true
	}

	c.Body = func(*GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyEnumType(d.Names)
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		var i int64
		if signed {
			i = v.Int()
		} else {
			i = int64(v.Uint())
		}
		if i < 0 || int(i) >= len(d.Names) {
			panic(&EncodeError{fmt.Sprintf("%s value %d is outside the declared enum range 0..%d",
				c.Typ, i, len(d.Names)-1)})
		}
		return b.Push(types.MakeSchemaValueNodeEnumValue(uint32(i)))
	}
	c.Decode = func(dec *Decoder, dst reflect.Value, idx int32) error {
		n, err := dec.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeEnumValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		i := n.EnumValue()
		if int(i) >= len(d.Names) {
			return fmt.Errorf("%s: enum index %d out of range (%d names)", c.Typ, i, len(d.Names))
		}
		if signed {
			dst.SetInt(int64(i))
		} else {
			dst.SetUint(uint64(i))
		}
		return nil
	}
}

// ---------------------------------------------------------------------------
// named types and markers
// ---------------------------------------------------------------------------

// namedTypeCodecs holds the types recognised by identity rather than by kind:
// each has a Go representation indistinguishable from a primitive, so only the
// named type reveals the intent.
var namedTypeCodecs = map[reflect.Type]func(*Codec){
	reflect.TypeFor[values.Char](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyCharType(), types.SchemaValueNodeCharValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodeCharValue(rune(v.Int())))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetInt(int64(n.CharValue())) })
	},
	reflect.TypeFor[values.URL](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyUrlType(types.UrlRestrictions{
			AllowedSchemes: witTypes.None[[]string](),
			AllowedHosts:   witTypes.None[[]string](),
		}), types.SchemaValueNodeUrlValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodeUrlValue(v.String()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetString(n.UrlValue()) })
	},
	reflect.TypeFor[time.Time](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyDatetimeType(), types.SchemaValueNodeDatetimeValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				t := v.Interface().(time.Time)
				return b.Push(types.MakeSchemaValueNodeDatetimeValue(types.Datetime{
					Seconds:     t.Unix(),
					Nanoseconds: uint32(t.Nanosecond()),
				}))
			},
			func(dst reflect.Value, n types.SchemaValueNode) {
				d := n.DatetimeValue()
				// UTC, so a round trip is stable: the wire carries no zone.
				dst.Set(reflect.ValueOf(time.Unix(d.Seconds, int64(d.Nanoseconds)).UTC()))
			})
	},
	reflect.TypeFor[values.Text](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyTextType(types.TextRestrictions{
			Languages: witTypes.None[[]string](),
			MinLength: witTypes.None[uint32](),
			MaxLength: witTypes.None[uint32](),
			Regex:     witTypes.None[string](),
		}), types.SchemaValueNodeTextValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodeTextValue(types.TextValuePayload{
					Text:     v.String(),
					Language: witTypes.None[string](),
				}))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetString(n.TextValue().Text) })
	},
	reflect.TypeFor[values.Binary](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyBinaryType(types.BinaryRestrictions{
			MimeTypes: witTypes.None[[]string](),
			MinBytes:  witTypes.None[uint32](),
			MaxBytes:  witTypes.None[uint32](),
		}), types.SchemaValueNodeBinaryValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodeBinaryValue(types.BinaryValuePayload{
					Bytes:    append([]uint8(nil), v.Bytes()...),
					MimeType: witTypes.None[string](),
				}))
			},
			func(dst reflect.Value, n types.SchemaValueNode) {
				dst.SetBytes(append([]byte(nil), n.BinaryValue().Bytes...))
			})
	},
	reflect.TypeFor[values.Path](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyPathType(types.PathSpec{
			Direction:         types.PathDirectionInOut,
			Kind:              types.PathKindAny,
			AllowedMimeTypes:  witTypes.None[[]string](),
			AllowedExtensions: witTypes.None[[]string](),
		}), types.SchemaValueNodePathValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodePathValue(v.String()))
			},
			func(dst reflect.Value, n types.SchemaValueNode) { dst.SetString(n.PathValue()) })
	},
	reflect.TypeFor[time.Duration](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyDurationType(), types.SchemaValueNodeDurationValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodeDurationValue(
					types.DurationValuePayload{Nanoseconds: v.Int()}))
			},
			func(dst reflect.Value, n types.SchemaValueNode) {
				dst.SetInt(n.DurationValue().Nanoseconds)
			})
	},
	reflect.TypeFor[values.UUID](): func(c *Codec) {
		scalar(c, types.MakeSchemaTypeBodyUuidType(), types.SchemaValueNodeUuidValue,
			func(b *ValBuilder, v reflect.Value) int32 {
				return b.Push(types.MakeSchemaValueNodeUuidValue(UUIDToWit(v.Interface().(values.UUID))))
			},
			func(dst reflect.Value, n types.SchemaValueNode) {
				dst.Set(reflect.ValueOf(UUIDFromWit(n.UuidValue())))
			})
	},
}

// compileQuantity lowers Quantity[U] to the WIT quantity type. The unit marker U
// supplies the type-level constraints; the value supplies the fixed-point digits
// and the unit it is expressed in.
func compileQuantity(c *Codec, unit values.QuantityUnit) {
	suffixes := unit.AllowedSuffixes()
	c.Body = func(*GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyQuantityType(types.QuantitySpec{
			BaseUnit:        unit.BaseUnit(),
			AllowedSuffixes: append([]string(nil), suffixes...),
			Min:             witTypes.None[types.QuantityValue](),
			Max:             witTypes.None[types.QuantityValue](),
		})
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		mantissa, scale, u, _ := values.QuantityParts(v.Interface())
		return b.Push(types.MakeSchemaValueNodeQuantityValueNode(types.QuantityValue{
			Mantissa: mantissa,
			Scale:    scale,
			Unit:     u,
		}))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeQuantityValueNode {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		q := n.QuantityValueNode()
		values.QuantitySetParts(dst.Addr().Interface(), q.Mantissa, q.Scale, q.Unit)
		return nil
	}
}

// UUIDFromWit converts a wire UUID.
func UUIDFromWit(w types.Uuid) values.UUID {
	var u values.UUID
	binary.BigEndian.PutUint64(u[0:8], w.HighBits)
	binary.BigEndian.PutUint64(u[8:16], w.LowBits)
	return u
}

// UUIDToWit converts a UUID to its wire form.
func UUIDToWit(u values.UUID) types.Uuid {
	return types.Uuid{
		HighBits: binary.BigEndian.Uint64(u[0:8]),
		LowBits:  binary.BigEndian.Uint64(u[8:16]),
	}
}
