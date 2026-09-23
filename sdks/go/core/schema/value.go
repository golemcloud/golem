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

// Schema values.
//
// A value is recursive in the same way its type is: a record holds its fields,
// not indices into a pool. The guest SDK's flattened form converts at the
// boundary.

// SchemaValue is one value. The interface is closed: only the cases below
// implement it.
type SchemaValue interface{ isSchemaValue() }

// BoolValue is a boolean.
type BoolValue struct{ Value bool }

// S8Value is a s8 number.
type S8Value struct{ Value int8 }

// S16Value is a s16 number.
type S16Value struct{ Value int16 }

// S32Value is a s32 number.
type S32Value struct{ Value int32 }

// S64Value is a s64 number.
type S64Value struct{ Value int64 }

// U8Value is a u8 number.
type U8Value struct{ Value uint8 }

// U16Value is a u16 number.
type U16Value struct{ Value uint16 }

// U32Value is a u32 number.
type U32Value struct{ Value uint32 }

// U64Value is a u64 number.
type U64Value struct{ Value uint64 }

// F32Value is a f32 number.
type F32Value struct{ Value float32 }

// F64Value is a f64 number.
type F64Value struct{ Value float64 }

// CharValue is one Unicode code point.
type CharValue struct{ Value rune }

// StringValue is a string.
type StringValue struct{ Value string }

// RecordValue holds one value per declared field, in declaration order.
type RecordValue struct{ Fields []SchemaValue }

// VariantValue is the resolved case and its payload, which may be absent.
type VariantValue struct {
	Case    uint32
	Payload *SchemaValue
}

// EnumValue is the index of the selected case.
type EnumValue struct{ Case uint32 }

// FlagsValue holds one boolean per declared flag, in declaration order.
type FlagsValue struct{ Set []bool }

// TupleValue holds one value per element, in order.
type TupleValue struct{ Elements []SchemaValue }

// ListValue is a variable-length sequence.
type ListValue struct{ Items []SchemaValue }

// FixedListValue is a sequence whose length the type fixes.
type FixedListValue struct{ Items []SchemaValue }

// MapValue is a sequence of pairs, since keys need not be strings.
type MapValue struct{ Entries []MapEntry }

// MapEntry is one key-value pair.
type MapEntry struct {
	Key   SchemaValue
	Value SchemaValue
}

// OptionValue is a value that may be absent.
type OptionValue struct{ Value *SchemaValue }

// ResultValue is a success or a failure, either of which may carry no value.
type ResultValue struct {
	IsErr bool
	Value *SchemaValue
}

// TextValue is prose, with the language it is written in when known.
type TextValue struct {
	Text     string
	Language *string
}

// BinaryValue is bytes, with their media type when known.
type BinaryValue struct {
	Bytes    []byte
	MimeType *string
}

// PathValue is a filesystem path.
type PathValue struct{ Value string }

// UrlValue is a URL.
type UrlValue struct{ Value string }

// DatetimeValue is an instant, as seconds since the epoch plus nanoseconds.
type DatetimeValue struct {
	Seconds     int64
	Nanoseconds uint32
}

// DurationValue is a signed span in nanoseconds.
type DurationValue struct{ Nanoseconds int64 }

// QuantityValueNode is a measurement carrying its unit.
type QuantityValueNode struct{ Value QuantityValue }

// UnionValue is the resolved branch and its body. The tag travels so a receiver
// does not have to re-run the discriminator rules.
type UnionValue struct {
	Tag  string
	Body SchemaValue
}

// SecretValue is a handle to a value the platform holds.
type SecretValue struct{ Handle SecretHandle }

// QuotaTokenValue is a handle to a spending allowance.
type QuotaTokenValue struct{ Handle QuotaTokenHandle }

// PermissionCardValue is a handle to an authority the host granted.
type PermissionCardValue struct{ Handle PermissionCardHandle }

// StreamValue is a handle to a sequence that arrives over time.
type StreamValue struct{ Handle StreamHandle }

// The closed-interface markers.
func (BoolValue) isSchemaValue()           {}
func (S8Value) isSchemaValue()             {}
func (S16Value) isSchemaValue()            {}
func (S32Value) isSchemaValue()            {}
func (S64Value) isSchemaValue()            {}
func (U8Value) isSchemaValue()             {}
func (U16Value) isSchemaValue()            {}
func (U32Value) isSchemaValue()            {}
func (U64Value) isSchemaValue()            {}
func (F32Value) isSchemaValue()            {}
func (F64Value) isSchemaValue()            {}
func (CharValue) isSchemaValue()           {}
func (StringValue) isSchemaValue()         {}
func (RecordValue) isSchemaValue()         {}
func (VariantValue) isSchemaValue()        {}
func (EnumValue) isSchemaValue()           {}
func (FlagsValue) isSchemaValue()          {}
func (TupleValue) isSchemaValue()          {}
func (ListValue) isSchemaValue()           {}
func (FixedListValue) isSchemaValue()      {}
func (MapValue) isSchemaValue()            {}
func (OptionValue) isSchemaValue()         {}
func (ResultValue) isSchemaValue()         {}
func (TextValue) isSchemaValue()           {}
func (BinaryValue) isSchemaValue()         {}
func (PathValue) isSchemaValue()           {}
func (UrlValue) isSchemaValue()            {}
func (DatetimeValue) isSchemaValue()       {}
func (DurationValue) isSchemaValue()       {}
func (QuantityValueNode) isSchemaValue()   {}
func (UnionValue) isSchemaValue()          {}
func (SecretValue) isSchemaValue()         {}
func (QuotaTokenValue) isSchemaValue()     {}
func (PermissionCardValue) isSchemaValue() {}
func (StreamValue) isSchemaValue()         {}
