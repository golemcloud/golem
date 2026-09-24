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
	"fmt"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// Conversions between Go values and schema values, for generated clients.
//
// A generated external client has no reflective codec to lean on, so it
// converts explicitly. These are the pieces it composes: one pair per leaf
// kind, and one pair per composite that takes the element conversions as
// arguments. A generated record encoder is then a single expression per field,
// and a list of options of records is bridge.EncodeList(v, func(e …) …
// { return bridge.EncodeOption(e, encodeOrder) }).
//
// Decoding checks the kind of every node and returns an error naming what it
// expected, rather than trusting the server's value to match the schema the
// client was generated from.

// Encoder converts a Go value into a schema value.
type Encoder[T any] func(T) schema.SchemaValue

// Decoder converts a schema value into a Go value.
type Decoder[T any] func(schema.SchemaValue) (T, error)

// Mismatch reports a schema value of the wrong kind.
func Mismatch(want string, got schema.SchemaValue) error {
	return fmt.Errorf("golem: expected a %s value, got %T", want, got)
}

func decodeAs[V schema.SchemaValue, T any](want string, sv schema.SchemaValue, read func(V) T) (T, error) {
	v, ok := sv.(V)
	if !ok {
		var zero T
		return zero, Mismatch(want, sv)
	}
	return read(v), nil
}

// --- leaves ------------------------------------------------------------------

func EncodeBool(v bool) schema.SchemaValue { return schema.BoolValue{Value: v} }
func DecodeBool(sv schema.SchemaValue) (bool, error) {
	return decodeAs("bool", sv, func(v schema.BoolValue) bool { return v.Value })
}

func EncodeS8(v int8) schema.SchemaValue { return schema.S8Value{Value: v} }
func DecodeS8(sv schema.SchemaValue) (int8, error) {
	return decodeAs("s8", sv, func(v schema.S8Value) int8 { return v.Value })
}

func EncodeS16(v int16) schema.SchemaValue { return schema.S16Value{Value: v} }
func DecodeS16(sv schema.SchemaValue) (int16, error) {
	return decodeAs("s16", sv, func(v schema.S16Value) int16 { return v.Value })
}

func EncodeS32(v int32) schema.SchemaValue { return schema.S32Value{Value: v} }
func DecodeS32(sv schema.SchemaValue) (int32, error) {
	return decodeAs("s32", sv, func(v schema.S32Value) int32 { return v.Value })
}

func EncodeS64(v int64) schema.SchemaValue { return schema.S64Value{Value: v} }
func DecodeS64(sv schema.SchemaValue) (int64, error) {
	return decodeAs("s64", sv, func(v schema.S64Value) int64 { return v.Value })
}

func EncodeU8(v uint8) schema.SchemaValue { return schema.U8Value{Value: v} }
func DecodeU8(sv schema.SchemaValue) (uint8, error) {
	return decodeAs("u8", sv, func(v schema.U8Value) uint8 { return v.Value })
}

func EncodeU16(v uint16) schema.SchemaValue { return schema.U16Value{Value: v} }
func DecodeU16(sv schema.SchemaValue) (uint16, error) {
	return decodeAs("u16", sv, func(v schema.U16Value) uint16 { return v.Value })
}

func EncodeU32(v uint32) schema.SchemaValue { return schema.U32Value{Value: v} }
func DecodeU32(sv schema.SchemaValue) (uint32, error) {
	return decodeAs("u32", sv, func(v schema.U32Value) uint32 { return v.Value })
}

func EncodeU64(v uint64) schema.SchemaValue { return schema.U64Value{Value: v} }
func DecodeU64(sv schema.SchemaValue) (uint64, error) {
	return decodeAs("u64", sv, func(v schema.U64Value) uint64 { return v.Value })
}

func EncodeF32(v float32) schema.SchemaValue { return schema.F32Value{Value: v} }
func DecodeF32(sv schema.SchemaValue) (float32, error) {
	return decodeAs("f32", sv, func(v schema.F32Value) float32 { return v.Value })
}

func EncodeF64(v float64) schema.SchemaValue { return schema.F64Value{Value: v} }
func DecodeF64(sv schema.SchemaValue) (float64, error) {
	return decodeAs("f64", sv, func(v schema.F64Value) float64 { return v.Value })
}

func EncodeString(v string) schema.SchemaValue { return schema.StringValue{Value: v} }
func DecodeString(sv schema.SchemaValue) (string, error) {
	return decodeAs("string", sv, func(v schema.StringValue) string { return v.Value })
}

func EncodeChar(v values.Char) schema.SchemaValue { return schema.CharValue{Value: rune(v)} }
func DecodeChar(sv schema.SchemaValue) (values.Char, error) {
	return decodeAs("char", sv, func(v schema.CharValue) values.Char { return values.Char(v.Value) })
}

func EncodeText(v values.Text) schema.SchemaValue { return schema.TextValue{Text: string(v)} }
func DecodeText(sv schema.SchemaValue) (values.Text, error) {
	return decodeAs("text", sv, func(v schema.TextValue) values.Text { return values.Text(v.Text) })
}

func EncodeBinary(v values.Binary) schema.SchemaValue {
	return schema.BinaryValue{Bytes: []byte(v)}
}
func DecodeBinary(sv schema.SchemaValue) (values.Binary, error) {
	return decodeAs("binary", sv, func(v schema.BinaryValue) values.Binary { return values.Binary(v.Bytes) })
}

func EncodePath(v values.Path) schema.SchemaValue { return schema.PathValue{Value: string(v)} }
func DecodePath(sv schema.SchemaValue) (values.Path, error) {
	return decodeAs("path", sv, func(v schema.PathValue) values.Path { return values.Path(v.Value) })
}

func EncodeURL(v values.URL) schema.SchemaValue { return schema.UrlValue{Value: string(v)} }
func DecodeURL(sv schema.SchemaValue) (values.URL, error) {
	return decodeAs("url", sv, func(v schema.UrlValue) values.URL { return values.URL(v.Value) })
}

// EncodeDatetime carries an instant as seconds and nanoseconds since the epoch.
func EncodeDatetime(v time.Time) schema.SchemaValue {
	return schema.DatetimeValue{Seconds: v.Unix(), Nanoseconds: uint32(v.Nanosecond())}
}
func DecodeDatetime(sv schema.SchemaValue) (time.Time, error) {
	return decodeAs("datetime", sv, func(v schema.DatetimeValue) time.Time {
		return time.Unix(v.Seconds, int64(v.Nanoseconds)).UTC()
	})
}

func EncodeDuration(v time.Duration) schema.SchemaValue {
	return schema.DurationValue{Nanoseconds: int64(v)}
}
func DecodeDuration(sv schema.SchemaValue) (time.Duration, error) {
	return decodeAs("duration", sv, func(v schema.DurationValue) time.Duration {
		return time.Duration(v.Nanoseconds)
	})
}

// --- sequences ---------------------------------------------------------------

func EncodeList[T any](xs []T, f Encoder[T]) schema.SchemaValue {
	return schema.ListValue{Items: encodeAll(xs, f)}
}

func DecodeList[T any](sv schema.SchemaValue, f Decoder[T]) ([]T, error) {
	v, ok := sv.(schema.ListValue)
	if !ok {
		return nil, Mismatch("list", sv)
	}
	return decodeAll(v.Items, f)
}

// EncodeFixedList takes the array as a slice, since Go cannot abstract over an
// array's length; the caller passes arr[:].
func EncodeFixedList[T any](xs []T, f Encoder[T]) schema.SchemaValue {
	return schema.FixedListValue{Items: encodeAll(xs, f)}
}

// DecodeFixedList checks the declared length and returns the items as a slice
// for the caller to copy into its array.
func DecodeFixedList[T any](sv schema.SchemaValue, length int, f Decoder[T]) ([]T, error) {
	v, ok := sv.(schema.FixedListValue)
	if !ok {
		return nil, Mismatch("fixed-list", sv)
	}
	if len(v.Items) != length {
		return nil, fmt.Errorf("golem: expected a fixed list of %d items, got %d", length, len(v.Items))
	}
	return decodeAll(v.Items, f)
}

func EncodeMap[K comparable, V any](m map[K]V, fk Encoder[K], fv Encoder[V]) schema.SchemaValue {
	entries := make([]schema.MapEntry, 0, len(m))
	for k, v := range m {
		entries = append(entries, schema.MapEntry{Key: fk(k), Value: fv(v)})
	}
	return schema.MapValue{Entries: entries}
}

func DecodeMap[K comparable, V any](sv schema.SchemaValue, fk Decoder[K], fv Decoder[V]) (map[K]V, error) {
	v, ok := sv.(schema.MapValue)
	if !ok {
		return nil, Mismatch("map", sv)
	}
	out := make(map[K]V, len(v.Entries))
	for i, entry := range v.Entries {
		key, err := fk(entry.Key)
		if err != nil {
			return nil, fmt.Errorf("map key %d: %w", i, err)
		}
		value, err := fv(entry.Value)
		if err != nil {
			return nil, fmt.Errorf("map value %d: %w", i, err)
		}
		out[key] = value
	}
	return out, nil
}

func encodeAll[T any](xs []T, f Encoder[T]) []schema.SchemaValue {
	out := make([]schema.SchemaValue, len(xs))
	for i, x := range xs {
		out[i] = f(x)
	}
	return out
}

func decodeAll[T any](items []schema.SchemaValue, f Decoder[T]) ([]T, error) {
	out := make([]T, len(items))
	for i, item := range items {
		v, err := f(item)
		if err != nil {
			return nil, fmt.Errorf("item %d: %w", i, err)
		}
		out[i] = v
	}
	return out, nil
}

// --- option and result ---------------------------------------------------------

func EncodeOption[T any](o values.Option[T], f Encoder[T]) schema.SchemaValue {
	v, some := o.Get()
	if !some {
		return schema.OptionValue{}
	}
	inner := f(v)
	return schema.OptionValue{Value: &inner}
}

func DecodeOption[T any](sv schema.SchemaValue, f Decoder[T]) (values.Option[T], error) {
	v, ok := sv.(schema.OptionValue)
	if !ok {
		return values.None[T](), Mismatch("option", sv)
	}
	if v.Value == nil {
		return values.None[T](), nil
	}
	inner, err := f(*v.Value)
	if err != nil {
		return values.None[T](), err
	}
	return values.Some(inner), nil
}

// EncodeResult takes a nil encoder for an arm that carries no value, which the
// generated type spells struct{}.
func EncodeResult[O, E any](r values.Result[O, E], fo Encoder[O], fe Encoder[E]) schema.SchemaValue {
	if r.IsErr() {
		return schema.ResultValue{IsErr: true, Value: encodeArm(r.Err(), fe)}
	}
	return schema.ResultValue{Value: encodeArm(r.Ok(), fo)}
}

// DecodeResult takes a nil decoder for an arm that carries no value.
func DecodeResult[O, E any](sv schema.SchemaValue, fo Decoder[O], fe Decoder[E]) (values.Result[O, E], error) {
	v, ok := sv.(schema.ResultValue)
	if !ok {
		return values.Result[O, E]{}, Mismatch("result", sv)
	}
	if v.IsErr {
		e, err := decodeArm(v.Value, fe, "err")
		if err != nil {
			return values.Result[O, E]{}, err
		}
		return values.Err[O](e), nil
	}
	o, err := decodeArm(v.Value, fo, "ok")
	if err != nil {
		return values.Result[O, E]{}, err
	}
	return values.Ok[O, E](o), nil
}

func encodeArm[T any](v T, f Encoder[T]) *schema.SchemaValue {
	if f == nil {
		return nil
	}
	inner := f(v)
	return &inner
}

func decodeArm[T any](v *schema.SchemaValue, f Decoder[T], arm string) (T, error) {
	var zero T
	switch {
	case f == nil && v == nil:
		return zero, nil
	case f == nil:
		return zero, fmt.Errorf("golem: the %s arm declares no value but one arrived", arm)
	case v == nil:
		return zero, fmt.Errorf("golem: the %s arm carries no value", arm)
	}
	return f(*v)
}

// Tuples are in codec_tuple.go.

func tupleElements(sv schema.SchemaValue, arity int) ([]schema.SchemaValue, error) {
	v, ok := sv.(schema.TupleValue)
	if !ok {
		return nil, Mismatch("tuple", sv)
	}
	if len(v.Elements) != arity {
		return nil, fmt.Errorf("golem: expected a %d-tuple, got %d elements", arity, len(v.Elements))
	}
	return v.Elements, nil
}
