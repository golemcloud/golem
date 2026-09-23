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

// The schema model.
//
// This is the recursive form the REST and RPC surfaces use: a graph is a set of
// named definitions plus a root type, and a reference names a definition by its
// string id. The guest SDK's WebAssembly bindings carry a flattened variant of
// the same thing — one pool of nodes addressed by index — and convert at the
// boundary. Nothing here knows about that.
//
// A sum type is an interface with one struct per case, so a reader dispatches
// with an ordinary type switch and each case carries exactly its own fields.
// Optional values are pointers; nil means absent.

// SchemaGraph is a self-contained type: every reference in Root resolves
// against Defs.
type SchemaGraph struct {
	Defs []SchemaTypeDef
	Root SchemaType
}

// Def looks a named definition up by id.
func (g SchemaGraph) Def(id string) (SchemaTypeDef, bool) {
	for _, d := range g.Defs {
		if d.Id == id {
			return d, true
		}
	}
	return SchemaTypeDef{}, false
}

// SchemaTypeDef is a named definition. Naming a type is what lets it recur and
// what lets two languages agree on its identity.
type SchemaTypeDef struct {
	// Id is stable and language-independent; it is what a RefType names.
	Id string
	// Name is a human-readable qualified name, for display only.
	Name *string
	Body SchemaType
}

// SchemaType is one node: what it is, plus what is documented about it.
type SchemaType struct {
	Body     SchemaTypeBody
	Metadata MetadataEnvelope
}

// MetadataEnvelope is the documentation carried alongside a type.
type MetadataEnvelope struct {
	Doc      *string
	Aliases  []string
	Examples []string
	// Deprecated carries the reason; nil means not deprecated.
	Deprecated *string
	Role       *Role
}

// Role tags a type with a consumer-facing intent.
type Role string

const (
	RoleMultimodal         Role = "multimodal"
	RoleUnstructuredText   Role = "unstructured-text"
	RoleUnstructuredBinary Role = "unstructured-binary"
)

// SchemaTypeBody is the structural body of a type. The interface is closed:
// only the cases below implement it.
type SchemaTypeBody interface{ isSchemaTypeBody() }

// RefType names a definition in the enclosing graph. It is how a recursive type
// is expressed, and the only form of recursion the wire allows.
type RefType struct{ Id string }

// BoolType is a boolean.
type BoolType struct{}

// CharType is one Unicode code point.
type CharType struct{}

// StringType is an unconstrained string.
type StringType struct{}

// S8Type is a s8 number.
type S8Type struct{ Restrictions *NumericRestrictions }

// S16Type is a s16 number.
type S16Type struct{ Restrictions *NumericRestrictions }

// S32Type is a s32 number.
type S32Type struct{ Restrictions *NumericRestrictions }

// S64Type is a s64 number.
type S64Type struct{ Restrictions *NumericRestrictions }

// U8Type is a u8 number.
type U8Type struct{ Restrictions *NumericRestrictions }

// U16Type is a u16 number.
type U16Type struct{ Restrictions *NumericRestrictions }

// U32Type is a u32 number.
type U32Type struct{ Restrictions *NumericRestrictions }

// U64Type is a u64 number.
type U64Type struct{ Restrictions *NumericRestrictions }

// F32Type is a f32 number.
type F32Type struct{ Restrictions *NumericRestrictions }

// F64Type is a f64 number.
type F64Type struct{ Restrictions *NumericRestrictions }

// NumericRestrictions bounds a numeric type.
type NumericRestrictions struct {
	Min  *NumericBound
	Max  *NumericBound
	Unit *string
}

// NumericBound is one end of a numeric range, kept in the widest form that can
// hold it so no precision is lost on the way through.
type NumericBound struct {
	Kind      NumericBoundKind
	Signed    int64
	Unsigned  uint64
	FloatBits uint64
}

// NumericBoundKind says which field of a [NumericBound] is meaningful.
type NumericBoundKind uint8

const (
	BoundSigned NumericBoundKind = iota
	BoundUnsigned
	BoundFloatBits
)

// RecordType is a fixed set of named fields, in declaration order.
type RecordType struct{ Fields []NamedField }

// NamedField is one field of a record.
type NamedField struct {
	Name     string
	Body     SchemaType
	Metadata MetadataEnvelope
}

// VariantType is a tagged sum: the case travels with the value.
type VariantType struct{ Cases []VariantCase }

// VariantCase is one case of a variant. A nil Payload carries no value.
type VariantCase struct {
	Name     string
	Payload  *SchemaType
	Metadata MetadataEnvelope
}

// EnumType is a closed set of names with no payloads.
type EnumType struct{ Cases []string }

// FlagsType is a set of independent named booleans.
type FlagsType struct{ Flags []string }

// TupleType is a positional group with no field names.
type TupleType struct{ Elements []SchemaType }

// ListType is a variable-length sequence.
type ListType struct{ Element SchemaType }

// FixedListType is a sequence of exactly Length elements.
type FixedListType struct {
	Element SchemaType
	Length  uint32
}

// MapType is a set of key-value pairs. Keys are not restricted to strings,
// which is why a map does not travel as a JSON object.
type MapType struct {
	Key   SchemaType
	Value SchemaType
}

// OptionType is a value that may be absent.
type OptionType struct{ Inner SchemaType }

// ResultType is a success or a failure, either of which may carry no value.
type ResultType struct {
	Ok  *SchemaType
	Err *SchemaType
}

// TextType is human-language prose, as distinct from an identifier.
type TextType struct{ Restrictions TextRestrictions }

// TextRestrictions bounds a text value.
type TextRestrictions struct {
	Languages *[]string
	MinLength *uint32
	MaxLength *uint32
	Regex     *string
}

// BinaryType is an opaque byte payload.
type BinaryType struct{ Restrictions BinaryRestrictions }

// BinaryRestrictions bounds a binary value. The byte counts are of the raw
// bytes, not of their encoded form.
type BinaryRestrictions struct {
	MimeTypes *[]string
	MinBytes  *uint32
	MaxBytes  *uint32
}

// PathType is a filesystem path exchanged with the host.
type PathType struct{ Spec PathSpec }

// PathSpec says which way a path is used and what may live at it.
type PathSpec struct {
	Direction         PathDirection
	Kind              PathKind
	AllowedMimeTypes  *[]string
	AllowedExtensions *[]string
}

// PathDirection says whether a path is read, written, or both.
type PathDirection uint8

const (
	PathInput PathDirection = iota
	PathOutput
	PathInOut
)

// PathKind says what may live at a path.
type PathKind uint8

const (
	PathFile PathKind = iota
	PathDirectory
	PathAny
)

// UrlType is a URL.
type UrlType struct{ Restrictions UrlRestrictions }

// UrlRestrictions bounds a URL.
type UrlRestrictions struct {
	AllowedSchemes *[]string
	AllowedHosts   *[]string
}

// DatetimeType is an instant.
type DatetimeType struct{}

// DurationType is a signed span, carried as nanoseconds.
type DurationType struct{}

// QuantityType is a fixed-point measurement carrying its unit.
type QuantityType struct{ Spec QuantitySpec }

// QuantitySpec constrains the units a quantity may be expressed in.
type QuantitySpec struct {
	BaseUnit        string
	AllowedSuffixes []string
	Min             *QuantityValue
	Max             *QuantityValue
}

// QuantityValue is Mantissa x 10^-Scale, in Unit.
type QuantityValue struct {
	Mantissa int64
	Scale    int32
	Unit     string
}

// UnionType is an inferred-tag sum: the branch is recognised from the value
// rather than carried beside it.
type UnionType struct{ Branches []UnionBranch }

// UnionBranch is one branch of a union.
type UnionBranch struct {
	Tag           string
	Body          SchemaType
	Discriminator DiscriminatorRule
	Metadata      MetadataEnvelope
}

// DiscriminatorRule recognises a union branch from a raw value.
type DiscriminatorRule interface{ isDiscriminatorRule() }

// PrefixRule matches a string starting with Value.
type PrefixRule struct{ Value string }

// SuffixRule matches a string ending with Value.
type SuffixRule struct{ Value string }

// ContainsRule matches a string containing Value.
type ContainsRule struct{ Value string }

// RegexRule matches a string against a pattern.
type RegexRule struct{ Pattern string }

// FieldEqualsRule matches a record carrying FieldName, optionally with a given
// literal value.
type FieldEqualsRule struct {
	FieldName string
	Literal   *string
}

// FieldAbsentRule matches a record that does not carry FieldName.
type FieldAbsentRule struct{ FieldName string }

// SecretType is a handle to a value the platform holds.
type SecretType struct {
	Inner    SchemaType
	Category *string
}

// QuotaTokenType is a handle to a spending allowance.
type QuotaTokenType struct{ ResourceName *string }

// PermissionCardType is a handle to an authority the host granted.
type PermissionCardType struct{ Polymorphic bool }

// FutureType is a value that will arrive later. A nil Item means the future is
// untyped, which carries no item codec and cannot be used.
type FutureType struct{ Item *SchemaType }

// StreamType is a sequence that arrives over time, with the same rule about a
// nil Item.
type StreamType struct{ Item *SchemaType }

// The closed-interface markers.
func (RefType) isSchemaTypeBody()            {}
func (BoolType) isSchemaTypeBody()           {}
func (CharType) isSchemaTypeBody()           {}
func (StringType) isSchemaTypeBody()         {}
func (S8Type) isSchemaTypeBody()             {}
func (S16Type) isSchemaTypeBody()            {}
func (S32Type) isSchemaTypeBody()            {}
func (S64Type) isSchemaTypeBody()            {}
func (U8Type) isSchemaTypeBody()             {}
func (U16Type) isSchemaTypeBody()            {}
func (U32Type) isSchemaTypeBody()            {}
func (U64Type) isSchemaTypeBody()            {}
func (F32Type) isSchemaTypeBody()            {}
func (F64Type) isSchemaTypeBody()            {}
func (RecordType) isSchemaTypeBody()         {}
func (VariantType) isSchemaTypeBody()        {}
func (EnumType) isSchemaTypeBody()           {}
func (FlagsType) isSchemaTypeBody()          {}
func (TupleType) isSchemaTypeBody()          {}
func (ListType) isSchemaTypeBody()           {}
func (FixedListType) isSchemaTypeBody()      {}
func (MapType) isSchemaTypeBody()            {}
func (OptionType) isSchemaTypeBody()         {}
func (ResultType) isSchemaTypeBody()         {}
func (TextType) isSchemaTypeBody()           {}
func (BinaryType) isSchemaTypeBody()         {}
func (PathType) isSchemaTypeBody()           {}
func (UrlType) isSchemaTypeBody()            {}
func (DatetimeType) isSchemaTypeBody()       {}
func (DurationType) isSchemaTypeBody()       {}
func (QuantityType) isSchemaTypeBody()       {}
func (UnionType) isSchemaTypeBody()          {}
func (SecretType) isSchemaTypeBody()         {}
func (QuotaTokenType) isSchemaTypeBody()     {}
func (PermissionCardType) isSchemaTypeBody() {}
func (FutureType) isSchemaTypeBody()         {}
func (StreamType) isSchemaTypeBody()         {}

func (PrefixRule) isDiscriminatorRule()      {}
func (SuffixRule) isDiscriminatorRule()      {}
func (ContainsRule) isDiscriminatorRule()    {}
func (RegexRule) isDiscriminatorRule()       {}
func (FieldEqualsRule) isDiscriminatorRule() {}
func (FieldAbsentRule) isDiscriminatorRule() {}
