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

// Package postgres is a Go wrapper over Golem's durable Postgres client
// (golem:rdbms). Open a connection with a URL, then run parametrised statements
// in a database/sql-flavoured style:
//
//	db, err := postgres.Open("postgres://user:pass@host:5432/app")
//	rs, err := db.Query("SELECT id, name FROM users WHERE active = $1", true)
//	for _, row := range rs.Rows() {
//	    var id int64
//	    var name string
//	    row.Scan(&id, &name)
//	}
//
// Parameters are ordinary Go values: nil, bool, the int/float widths, string,
// []byte, [golem.UUID] and [time.Time] map to the natural Postgres types.
// Anything else — or a value whose exact Postgres type matters (numeric, jsonb,
// a specific integer width) — is built with the constructors in this package
// ([Numeric], [JSONB], [Int4], …), which return a [DbValue] you pass just like
// any other argument. Columns are read back positionally with the typed getters
// on [Row] ([Row.Int64], [Row.String], …), the generic [Row.Get], or [Row.Scan].
//
// Like the other WASI wrappers, operations return an error, distinct from the
// fail-loud control-flow surface. The connection is durable — every
// query/execute/commit is journaled and replayed — and because these are remote
// side effects, using it inside a read-only method traps.
//
// Every value family maps to a Go value: the common types (ints, floats, text,
// json, uuid, temporal, inet/cidr/macaddr, bit strings, vectors) and the
// composite families (arrays, composites, domains, ranges) through their typed
// [Row] getters and matching constructors.
//
// Pair a fallible call with golem.Must / golem.Must0 / golem.Must2 to abort the
// invocation on error.
package postgres

import (
	"encoding/binary"
	"fmt"
	"net/netip"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	pg "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_rdbms_postgres"
	rtypes "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_rdbms_types"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// ── Errors ──────────────────────────────────────────────────────────────────

// ErrorKind classifies an [Error].
type ErrorKind uint8

const (
	// ConnectionFailure means the connection could not be established or was lost.
	ConnectionFailure ErrorKind = iota
	// QueryParameterFailure means a parameter could not be encoded for the query.
	QueryParameterFailure
	// QueryExecutionFailure means the database rejected or failed the statement.
	QueryExecutionFailure
	// QueryResponseFailure means the result could not be decoded.
	QueryResponseFailure
	// Other is any error the host did not classify.
	Other
)

// Error is a Postgres host error.
type Error struct {
	Kind    ErrorKind
	Message string
}

func (e *Error) Error() string { return "golem/rdbms/postgres: " + e.Message }

func pgError(e pg.Error) error {
	switch e.Tag() {
	case pg.ErrorConnectionFailure:
		return &Error{Kind: ConnectionFailure, Message: e.ConnectionFailure()}
	case pg.ErrorQueryParameterFailure:
		return &Error{Kind: QueryParameterFailure, Message: e.QueryParameterFailure()}
	case pg.ErrorQueryExecutionFailure:
		return &Error{Kind: QueryExecutionFailure, Message: e.QueryExecutionFailure()}
	case pg.ErrorQueryResponseFailure:
		return &Error{Kind: QueryResponseFailure, Message: e.QueryResponseFailure()}
	default:
		return &Error{Kind: Other, Message: e.Other()}
	}
}

// ── DbValue (typed parameters) ───────────────────────────────────────────────

// DbValue is a Postgres value whose exact column type is chosen explicitly,
// rather than inferred from a Go value. Build one with a constructor in this
// package ([Numeric], [JSONB], [Int4], [Array], [Composite], …) and pass it as a
// query argument.
//
// A DbValue holds either a ready value (the flat constructors) or a deferred
// encoder (the recursive [Array]/[Composite]/[Domain]/[CustomRange], whose host
// resources are built only when the parameter is actually sent — so construction
// stays side-effect free and a bad element surfaces as a normal parameter error).
type DbValue struct {
	raw    pg.DbValue
	encode func() (pg.DbValue, error)
}

func flat(v pg.DbValue) DbValue { return DbValue{raw: v} }

// tagName gives a db-value family its display name (used in error text — never by
// encode/decode). It switches on the generated constants, not on the raw tag
// position, so a reordered or inserted WIT case still names correctly and a
// renamed or removed one fails to compile here. A newly appended WIT case shows
// as "unknown(N)" until a case and name are added below.
func tagName(tag uint8) string {
	switch tag {
	case pg.DbValueCharacter:
		return "character"
	case pg.DbValueInt2:
		return "int2"
	case pg.DbValueInt4:
		return "int4"
	case pg.DbValueInt8:
		return "int8"
	case pg.DbValueFloat4:
		return "float4"
	case pg.DbValueFloat8:
		return "float8"
	case pg.DbValueNumeric:
		return "numeric"
	case pg.DbValueBoolean:
		return "boolean"
	case pg.DbValueText:
		return "text"
	case pg.DbValueVarchar:
		return "varchar"
	case pg.DbValueBpchar:
		return "bpchar"
	case pg.DbValueTimestamp:
		return "timestamp"
	case pg.DbValueTimestamptz:
		return "timestamptz"
	case pg.DbValueDate:
		return "date"
	case pg.DbValueTime:
		return "time"
	case pg.DbValueTimetz:
		return "timetz"
	case pg.DbValueInterval:
		return "interval"
	case pg.DbValueBytea:
		return "bytea"
	case pg.DbValueJson:
		return "json"
	case pg.DbValueJsonb:
		return "jsonb"
	case pg.DbValueJsonpath:
		return "jsonpath"
	case pg.DbValueXml:
		return "xml"
	case pg.DbValueUuid:
		return "uuid"
	case pg.DbValueInet:
		return "inet"
	case pg.DbValueCidr:
		return "cidr"
	case pg.DbValueMacaddr:
		return "macaddr"
	case pg.DbValueBit:
		return "bit"
	case pg.DbValueVarbit:
		return "varbit"
	case pg.DbValueInt4range:
		return "int4range"
	case pg.DbValueInt8range:
		return "int8range"
	case pg.DbValueNumrange:
		return "numrange"
	case pg.DbValueTsrange:
		return "tsrange"
	case pg.DbValueTstzrange:
		return "tstzrange"
	case pg.DbValueDaterange:
		return "daterange"
	case pg.DbValueMoney:
		return "money"
	case pg.DbValueOid:
		return "oid"
	case pg.DbValueEnumeration:
		return "enumeration"
	case pg.DbValueComposite:
		return "composite"
	case pg.DbValueDomain:
		return "domain"
	case pg.DbValueArray:
		return "array"
	case pg.DbValueRange:
		return "range"
	case pg.DbValueNull:
		return "null"
	case pg.DbValueVector:
		return "vector"
	case pg.DbValueHalfvec:
		return "halfvec"
	case pg.DbValueSparsevec:
		return "sparsevec"
	default:
		return fmt.Sprintf("unknown(%d)", tag)
	}
}

// Null builds a SQL NULL parameter.
func Null() DbValue { return flat(pg.MakeDbValueNull()) }

// Char builds a Postgres "char" (single-byte integer) parameter.
func Char(v int8) DbValue { return flat(pg.MakeDbValueCharacter(v)) }

// Int2 builds a smallint parameter.
func Int2(v int16) DbValue { return flat(pg.MakeDbValueInt2(v)) }

// Int4 builds an integer parameter.
func Int4(v int32) DbValue { return flat(pg.MakeDbValueInt4(v)) }

// Int8 builds a bigint parameter.
func Int8(v int64) DbValue { return flat(pg.MakeDbValueInt8(v)) }

// Float4 builds a real parameter.
func Float4(v float32) DbValue { return flat(pg.MakeDbValueFloat4(v)) }

// Float8 builds a double precision parameter.
func Float8(v float64) DbValue { return flat(pg.MakeDbValueFloat8(v)) }

// Numeric builds an exact numeric/decimal parameter from its string form (so no
// precision is lost).
func Numeric(v string) DbValue { return flat(pg.MakeDbValueNumeric(v)) }

// Money builds a money parameter (value in the smallest currency unit).
func Money(v int64) DbValue { return flat(pg.MakeDbValueMoney(v)) }

// Oid builds an object identifier parameter.
func Oid(v uint32) DbValue { return flat(pg.MakeDbValueOid(v)) }

// Varchar builds a varchar parameter.
func Varchar(v string) DbValue { return flat(pg.MakeDbValueVarchar(v)) }

// Bpchar builds a blank-padded char parameter.
func Bpchar(v string) DbValue { return flat(pg.MakeDbValueBpchar(v)) }

// JSON builds a json parameter from its serialized text.
func JSON(v string) DbValue { return flat(pg.MakeDbValueJson(v)) }

// JSONB builds a jsonb parameter from its serialized text.
func JSONB(v string) DbValue { return flat(pg.MakeDbValueJsonb(v)) }

// JSONPath builds a jsonpath parameter.
func JSONPath(v string) DbValue { return flat(pg.MakeDbValueJsonpath(v)) }

// XML builds an xml parameter.
func XML(v string) DbValue { return flat(pg.MakeDbValueXml(v)) }

// Bit builds a fixed-length bit-string parameter.
func Bit(v []bool) DbValue { return flat(pg.MakeDbValueBit(v)) }

// Varbit builds a varying bit-string parameter.
func Varbit(v []bool) DbValue { return flat(pg.MakeDbValueVarbit(v)) }

// Inet builds an inet (host address) parameter.
func Inet(a netip.Addr) DbValue { return flat(pg.MakeDbValueInet(ipToWit(a))) }

// Cidr builds a cidr (network address) parameter.
func Cidr(a netip.Addr) DbValue { return flat(pg.MakeDbValueCidr(ipToWit(a))) }

// Macaddr builds a macaddr parameter.
func Macaddr(m types.MacAddr) DbValue { return flat(pg.MakeDbValueMacaddr(macToWit(m))) }

// Interval builds an interval parameter.
func Interval(iv types.Interval) DbValue {
	return flat(pg.MakeDbValueInterval(pg.Interval{
		Months: int32(iv.Months), Days: int32(iv.Days), Microseconds: iv.Microseconds,
	}))
}

// Date builds a date parameter.
func Date(d types.Date) DbValue {
	return flat(pg.MakeDbValueDate(pg.Date{Year: int32(d.Year), Month: uint8(d.Month), Day: uint8(d.Day)}))
}

// Time builds a time-of-day parameter.
func Time(t types.Time) DbValue { return flat(pg.MakeDbValueTime(typesTimeToWit(t))) }

// Timetz builds a time-of-day-with-offset parameter.
func Timetz(t types.Timetz) DbValue {
	return flat(pg.MakeDbValueTimetz(pg.Timetz{Time: typesTimeToWit(t.Time), Offset: int32(t.OffsetSeconds)}))
}

// Timestamp builds a timestamp (no time zone) parameter.
func Timestamp(ts types.Timestamp) DbValue {
	return flat(pg.MakeDbValueTimestamp(typesTimestampToWit(ts)))
}

// Timestamptz builds a timestamp-with-time-zone parameter.
func Timestamptz(ts types.Timestamptz) DbValue {
	return flat(pg.MakeDbValueTimestamptz(pg.Timestamptz{
		Timestamp: typesTimestampToWit(ts.Timestamp), Offset: int32(ts.OffsetSeconds),
	}))
}

// Vector builds a pgvector vector parameter.
func Vector(v []float32) DbValue { return flat(pg.MakeDbValueVector(v)) }

// Halfvec builds a pgvector halfvec parameter.
func Halfvec(v []float32) DbValue { return flat(pg.MakeDbValueHalfvec(v)) }

// Enumeration builds an enum parameter carrying the enum type name and the
// selected label.
func Enumeration(name, value string) DbValue {
	return flat(pg.MakeDbValueEnumeration(pg.Enumeration{Name: name, Value: value}))
}

// SparseVector builds a pgvector sparse-vector parameter.
func SparseVector(v SparseVec) DbValue {
	return flat(pg.MakeDbValueSparsevec(pg.SparseVec{Dim: int32(v.Dim), Indices: v.Indices, Values: v.Values}))
}

// ── Ranges ────────────────────────────────────────────────────────────────────

// BoundKind is the kind of a range [Bound].
type BoundKind uint8

const (
	// BoundUnbounded is an open end (-infinity / +infinity).
	BoundUnbounded BoundKind = iota
	// BoundIncluded is a closed end (the value is part of the range).
	BoundIncluded
	// BoundExcluded is an open end (the value is not part of the range).
	BoundExcluded
)

// Bound is one end of a [Range].
type Bound[T any] struct {
	Kind  BoundKind
	Value T // ignored when Kind is BoundUnbounded
}

// Included builds a closed bound.
func Included[T any](v T) Bound[T] { return Bound[T]{Kind: BoundIncluded, Value: v} }

// Excluded builds an open bound.
func Excluded[T any](v T) Bound[T] { return Bound[T]{Kind: BoundExcluded, Value: v} }

// Unbounded builds an infinite bound.
func Unbounded[T any]() Bound[T] { return Bound[T]{Kind: BoundUnbounded} }

// Range is a range value with a typed start and end.
type Range[T any] struct {
	Start Bound[T]
	End   Bound[T]
}

// Int4Range builds an int4range parameter. Reads back as Range[int32].
func Int4Range(r Range[int32]) DbValue {
	return flat(pg.MakeDbValueInt4range(pg.Int4range{Start: int4BoundToWit(r.Start), End: int4BoundToWit(r.End)}))
}

// Int8Range builds an int8range parameter. Reads back as Range[int64].
func Int8Range(r Range[int64]) DbValue {
	return flat(pg.MakeDbValueInt8range(pg.Int8range{Start: int8BoundToWit(r.Start), End: int8BoundToWit(r.End)}))
}

// NumRange builds a numrange parameter (bounds as numeric strings). Reads back as Range[string].
func NumRange(r Range[string]) DbValue {
	return flat(pg.MakeDbValueNumrange(pg.Numrange{Start: numBoundToWit(r.Start), End: numBoundToWit(r.End)}))
}

// TsRange builds a tsrange parameter. Reads back as Range[time.Time] (UTC).
func TsRange(r Range[time.Time]) DbValue {
	return flat(pg.MakeDbValueTsrange(pg.Tsrange{Start: tsBoundToWit(r.Start), End: tsBoundToWit(r.End)}))
}

// TstzRange builds a tstzrange parameter. Reads back as Range[time.Time] (offset preserved).
func TstzRange(r Range[time.Time]) DbValue {
	return flat(pg.MakeDbValueTstzrange(pg.Tstzrange{Start: tstzBoundToWit(r.Start), End: tstzBoundToWit(r.End)}))
}

// DateRange builds a daterange parameter. Reads back as Range[time.Time] (UTC, date only).
func DateRange(r Range[time.Time]) DbValue {
	return flat(pg.MakeDbValueDaterange(pg.Daterange{Start: dateBoundToWit(r.Start), End: dateBoundToWit(r.End)}))
}

// ── Recursive families (arrays, composites, domains, custom ranges) ──────────
//
// These are backed by host lazy-value resources built when the parameter is sent,
// so their constructors defer the work into DbValue.encode.

// Array builds an array parameter from elements, each an ordinary Go value or a
// nested [DbValue]. Reads back as []any.
func Array(elems ...any) DbValue {
	return DbValue{encode: func() (pg.DbValue, error) {
		lazies := make([]*pg.LazyDbValue, len(elems))
		for i, e := range elems {
			ev, err := encodeParam(e)
			if err != nil {
				return pg.DbValue{}, fmt.Errorf("array element %d: %w", i+1, err)
			}
			lazies[i] = pg.MakeLazyDbValue(ev)
		}
		return pg.MakeDbValueArray(lazies), nil
	}}
}

// Composite builds a composite (row) parameter of the named type with the given
// ordered field values. Reads back as [CompositeValue].
func Composite(name string, fields ...any) DbValue {
	return DbValue{encode: func() (pg.DbValue, error) {
		vals := make([]*pg.LazyDbValue, len(fields))
		for i, f := range fields {
			fv, err := encodeParam(f)
			if err != nil {
				return pg.DbValue{}, fmt.Errorf("composite field %d: %w", i+1, err)
			}
			vals[i] = pg.MakeLazyDbValue(fv)
		}
		return pg.MakeDbValueComposite(pg.Composite{Name: name, Values: vals}), nil
	}}
}

// Domain builds a domain parameter of the named type wrapping value. Reads back
// as [DomainValue].
func Domain(name string, value any) DbValue {
	return DbValue{encode: func() (pg.DbValue, error) {
		v, err := encodeParam(value)
		if err != nil {
			return pg.DbValue{}, fmt.Errorf("domain value: %w", err)
		}
		return pg.MakeDbValueDomain(pg.Domain{Name: name, Value: pg.MakeLazyDbValue(v)}), nil
	}}
}

// CustomRange builds a value of a user-defined range type (name) with arbitrary
// element bounds. Reads back as [RangeValue]. Use the typed [Int4Range] etc. for
// the built-in range types.
func CustomRange(name string, start, end Bound[any]) DbValue {
	return DbValue{encode: func() (pg.DbValue, error) {
		s, err := valueBoundToWit(start)
		if err != nil {
			return pg.DbValue{}, fmt.Errorf("range start: %w", err)
		}
		e, err := valueBoundToWit(end)
		if err != nil {
			return pg.DbValue{}, fmt.Errorf("range end: %w", err)
		}
		return pg.MakeDbValueRange(pg.Range{Name: name, Value: pg.ValuesRange{Start: s, End: e}}), nil
	}}
}

// Decoded shapes for the recursive families.

// Enum is the decoded form of an enumeration value.
type Enum struct{ Name, Value string }

// SparseVec is the decoded form of a sparse vector (and the input to [SparseVector]).
type SparseVec struct {
	Dim     int
	Indices []int32
	Values  []float32
}

// CompositeValue is the decoded form of a composite value.
type CompositeValue struct {
	Name   string
	Fields []any
}

// DomainValue is the decoded form of a domain value.
type DomainValue struct {
	Name  string
	Value any
}

// RangeValue is the decoded form of a user-defined range value.
type RangeValue struct {
	Name  string
	Start Bound[any]
	End   Bound[any]
}

// ── Parameter encoding ───────────────────────────────────────────────────────

func encodeParam(v any) (pg.DbValue, error) {
	switch x := v.(type) {
	case nil:
		return pg.MakeDbValueNull(), nil
	case DbValue:
		if x.encode != nil {
			return x.encode()
		}
		return x.raw, nil
	case bool:
		return pg.MakeDbValueBoolean(x), nil
	case int:
		return pg.MakeDbValueInt8(int64(x)), nil
	case int64:
		return pg.MakeDbValueInt8(x), nil
	case int32:
		return pg.MakeDbValueInt4(x), nil
	case int16:
		return pg.MakeDbValueInt2(x), nil
	case int8:
		return pg.MakeDbValueInt2(int16(x)), nil
	case float64:
		return pg.MakeDbValueFloat8(x), nil
	case float32:
		return pg.MakeDbValueFloat4(x), nil
	case string:
		return pg.MakeDbValueText(x), nil
	case []byte:
		return pg.MakeDbValueBytea(x), nil
	case golem.UUID:
		return pg.MakeDbValueUuid(uuidToWit(x)), nil
	case time.Time:
		return pg.MakeDbValueTimestamptz(goTimeToTimestamptz(x)), nil
	default:
		return pg.DbValue{}, fmt.Errorf("unsupported parameter type %T; wrap it with a postgres.* constructor", v)
	}
}

func encodeParams(args []any) ([]pg.DbValue, error) {
	if len(args) == 0 {
		return nil, nil
	}
	out := make([]pg.DbValue, len(args))
	for i, a := range args {
		v, err := encodeParam(a)
		if err != nil {
			return nil, fmt.Errorf("golem/rdbms/postgres: parameter %d: %w", i+1, err)
		}
		out[i] = v
	}
	return out, nil
}

// ── Value decoding ────────────────────────────────────────────────────────────
//
// decode is split so the pure part (decodeFlat, every non-recursive family) is
// natively testable, while the host-backed recursive part (decodeLazy, which
// calls LazyDbValue.Get/Drop) stays reachable only from the query paths.

func decode(v pg.DbValue) any {
	if x, ok := decodeFlat(v); ok {
		return x
	}
	return decodeLazy(v)
}

// decodeLazy handles the recursive families (array, composite, domain, custom
// range). It calls the host to read each lazy child and drops it afterwards.
func decodeLazy(v pg.DbValue) any {
	switch v.Tag() {
	case pg.DbValueArray:
		lazies := v.Array()
		out := make([]any, len(lazies))
		for i, l := range lazies {
			out[i] = decode(l.Get())
			l.Drop()
		}
		return out
	case pg.DbValueComposite:
		c := v.Composite()
		fields := make([]any, len(c.Values))
		for i, l := range c.Values {
			fields[i] = decode(l.Get())
			l.Drop()
		}
		return CompositeValue{Name: c.Name, Fields: fields}
	case pg.DbValueDomain:
		d := v.Domain()
		val := decode(d.Value.Get())
		d.Value.Drop()
		return DomainValue{Name: d.Name, Value: val}
	case pg.DbValueRange:
		rg := v.Range()
		return RangeValue{Name: rg.Name, Start: valueBoundFromWit(rg.Value.Start), End: valueBoundFromWit(rg.Value.End)}
	default:
		// A newly appended WIT family we do not decode yet.
		return fmt.Sprintf("unsupported(%s)", tagName(v.Tag()))
	}
}

func valueBoundFromWit(b pg.ValueBound) Bound[any] {
	switch b.Tag() {
	case pg.ValueBoundIncluded:
		l := b.Included()
		val := decode(l.Get())
		l.Drop()
		return Included[any](val)
	case pg.ValueBoundExcluded:
		l := b.Excluded()
		val := decode(l.Get())
		l.Drop()
		return Excluded[any](val)
	default:
		return Unbounded[any]()
	}
}

// decodeFlat handles every non-recursive family; ok is false for the recursive
// ones (array/composite/domain/range), which decodeLazy takes.
func decodeFlat(v pg.DbValue) (any, bool) {
	switch v.Tag() {
	case pg.DbValueNull:
		return nil, true
	case pg.DbValueCharacter:
		return v.Character(), true
	case pg.DbValueInt2:
		return v.Int2(), true
	case pg.DbValueInt4:
		return v.Int4(), true
	case pg.DbValueInt8:
		return v.Int8(), true
	case pg.DbValueFloat4:
		return v.Float4(), true
	case pg.DbValueFloat8:
		return v.Float8(), true
	case pg.DbValueNumeric:
		return v.Numeric(), true
	case pg.DbValueBoolean:
		return v.Boolean(), true
	case pg.DbValueText:
		return v.Text(), true
	case pg.DbValueVarchar:
		return v.Varchar(), true
	case pg.DbValueBpchar:
		return v.Bpchar(), true
	case pg.DbValueTimestamp:
		return timestampToGoTime(v.Timestamp(), time.UTC), true
	case pg.DbValueTimestamptz:
		return timestamptzToGoTime(v.Timestamptz()), true
	case pg.DbValueDate:
		return dateToGoTime(v.Date()), true
	case pg.DbValueTime:
		return witTimeToTypes(v.Time()), true
	case pg.DbValueTimetz:
		return types.Timetz{Time: witTimeToTypes(v.Timetz().Time), OffsetSeconds: int(v.Timetz().Offset)}, true
	case pg.DbValueInterval:
		iv := v.Interval()
		return types.Interval{Months: int(iv.Months), Days: int(iv.Days), Microseconds: iv.Microseconds}, true
	case pg.DbValueBytea:
		return v.Bytea(), true
	case pg.DbValueJson:
		return v.Json(), true
	case pg.DbValueJsonb:
		return v.Jsonb(), true
	case pg.DbValueJsonpath:
		return v.Jsonpath(), true
	case pg.DbValueXml:
		return v.Xml(), true
	case pg.DbValueUuid:
		return uuidFromWit(v.Uuid()), true
	case pg.DbValueInet:
		return ipFromWit(v.Inet()), true
	case pg.DbValueCidr:
		return ipFromWit(v.Cidr()), true
	case pg.DbValueMacaddr:
		return macFromWit(v.Macaddr()), true
	case pg.DbValueBit:
		return v.Bit(), true
	case pg.DbValueVarbit:
		return v.Varbit(), true
	case pg.DbValueMoney:
		return v.Money(), true
	case pg.DbValueOid:
		return v.Oid(), true
	case pg.DbValueVector:
		return v.Vector(), true
	case pg.DbValueHalfvec:
		return v.Halfvec(), true
	case pg.DbValueInt4range:
		return int4RangeFromWit(v.Int4range()), true
	case pg.DbValueInt8range:
		return int8RangeFromWit(v.Int8range()), true
	case pg.DbValueNumrange:
		return numRangeFromWit(v.Numrange()), true
	case pg.DbValueTsrange:
		return tsRangeFromWit(v.Tsrange()), true
	case pg.DbValueTstzrange:
		return tstzRangeFromWit(v.Tstzrange()), true
	case pg.DbValueDaterange:
		return dateRangeFromWit(v.Daterange()), true
	case pg.DbValueEnumeration:
		e := v.Enumeration()
		return Enum{Name: e.Name, Value: e.Value}, true
	case pg.DbValueSparsevec:
		s := v.Sparsevec()
		return SparseVec{Dim: int(s.Dim), Indices: s.Indices, Values: s.Values}, true
	default:
		// array, composite, domain, range — recursive, handled by decodeLazy.
		return nil, false
	}
}

// ── UUID / temporal / net conversions ────────────────────────────────────────

func uuidToWit(u golem.UUID) pg.Uuid {
	return pg.Uuid{
		HighBits: binary.BigEndian.Uint64(u[0:8]),
		LowBits:  binary.BigEndian.Uint64(u[8:16]),
	}
}

func uuidFromWit(w pg.Uuid) golem.UUID {
	var u golem.UUID
	binary.BigEndian.PutUint64(u[0:8], w.HighBits)
	binary.BigEndian.PutUint64(u[8:16], w.LowBits)
	return u
}

func goTimeToTimestamp(t time.Time) pg.Timestamp {
	return pg.Timestamp{
		Date: pg.Date{Year: int32(t.Year()), Month: uint8(t.Month()), Day: uint8(t.Day())},
		Time: pg.Time{Hour: uint8(t.Hour()), Minute: uint8(t.Minute()), Second: uint8(t.Second()), Nanosecond: uint32(t.Nanosecond())},
	}
}

func goTimeToTimestamptz(t time.Time) pg.Timestamptz {
	_, off := t.Zone()
	return pg.Timestamptz{Timestamp: goTimeToTimestamp(t), Offset: int32(off)}
}

func timestampToGoTime(ts pg.Timestamp, loc *time.Location) time.Time {
	return time.Date(int(ts.Date.Year), time.Month(ts.Date.Month), int(ts.Date.Day),
		int(ts.Time.Hour), int(ts.Time.Minute), int(ts.Time.Second), int(ts.Time.Nanosecond), loc)
}

func timestamptzToGoTime(ts pg.Timestamptz) time.Time {
	return timestampToGoTime(ts.Timestamp, time.FixedZone("", int(ts.Offset)))
}

func dateToGoTime(d pg.Date) time.Time {
	return time.Date(int(d.Year), time.Month(d.Month), int(d.Day), 0, 0, 0, 0, time.UTC)
}

func witTimeToTypes(t pg.Time) types.Time {
	return types.Time{Hour: int(t.Hour), Minute: int(t.Minute), Second: int(t.Second), Nanosecond: int(t.Nanosecond)}
}

func typesTimeToWit(t types.Time) pg.Time {
	return pg.Time{Hour: uint8(t.Hour), Minute: uint8(t.Minute), Second: uint8(t.Second), Nanosecond: uint32(t.Nanosecond)}
}

func typesTimestampToWit(ts types.Timestamp) pg.Timestamp {
	return pg.Timestamp{
		Date: pg.Date{Year: int32(ts.Date.Year), Month: uint8(ts.Date.Month), Day: uint8(ts.Date.Day)},
		Time: typesTimeToWit(ts.Time),
	}
}

func ipToWit(a netip.Addr) rtypes.IpAddress {
	if a.Is4() {
		b := a.As4()
		return rtypes.MakeIpAddressIpv4(witTypes.Tuple4[uint8, uint8, uint8, uint8]{F0: b[0], F1: b[1], F2: b[2], F3: b[3]})
	}
	b := a.As16()
	g := func(i int) uint16 { return uint16(b[i])<<8 | uint16(b[i+1]) }
	return rtypes.MakeIpAddressIpv6(witTypes.Tuple8[uint16, uint16, uint16, uint16, uint16, uint16, uint16, uint16]{
		F0: g(0), F1: g(2), F2: g(4), F3: g(6), F4: g(8), F5: g(10), F6: g(12), F7: g(14),
	})
}

func ipFromWit(a rtypes.IpAddress) netip.Addr {
	if a.Tag() == rtypes.IpAddressIpv4 {
		t := a.Ipv4()
		return netip.AddrFrom4([4]byte{t.F0, t.F1, t.F2, t.F3})
	}
	t := a.Ipv6()
	var b [16]byte
	put := func(i int, v uint16) { b[i] = byte(v >> 8); b[i+1] = byte(v) }
	put(0, t.F0)
	put(2, t.F1)
	put(4, t.F2)
	put(6, t.F3)
	put(8, t.F4)
	put(10, t.F5)
	put(12, t.F6)
	put(14, t.F7)
	return netip.AddrFrom16(b)
}

func macToWit(m types.MacAddr) rtypes.MacAddress {
	return rtypes.MacAddress{Octets: witTypes.Tuple6[uint8, uint8, uint8, uint8, uint8, uint8]{
		F0: m[0], F1: m[1], F2: m[2], F3: m[3], F4: m[4], F5: m[5],
	}}
}

func macFromWit(m rtypes.MacAddress) types.MacAddr {
	o := m.Octets
	return types.MacAddr{o.F0, o.F1, o.F2, o.F3, o.F4, o.F5}
}

func goTimeToDate(t time.Time) pg.Date {
	return pg.Date{Year: int32(t.Year()), Month: uint8(t.Month()), Day: uint8(t.Day())}
}

// ── Range bound conversions ───────────────────────────────────────────────────

func int4BoundToWit(b Bound[int32]) pg.Int4bound {
	switch b.Kind {
	case BoundIncluded:
		return pg.MakeInt4boundIncluded(b.Value)
	case BoundExcluded:
		return pg.MakeInt4boundExcluded(b.Value)
	default:
		return pg.MakeInt4boundUnbounded()
	}
}

func int4BoundFromWit(b pg.Int4bound) Bound[int32] {
	switch b.Tag() {
	case pg.Int4boundIncluded:
		return Included(b.Included())
	case pg.Int4boundExcluded:
		return Excluded(b.Excluded())
	default:
		return Unbounded[int32]()
	}
}

func int4RangeFromWit(r pg.Int4range) Range[int32] {
	return Range[int32]{Start: int4BoundFromWit(r.Start), End: int4BoundFromWit(r.End)}
}

func int8BoundToWit(b Bound[int64]) pg.Int8bound {
	switch b.Kind {
	case BoundIncluded:
		return pg.MakeInt8boundIncluded(b.Value)
	case BoundExcluded:
		return pg.MakeInt8boundExcluded(b.Value)
	default:
		return pg.MakeInt8boundUnbounded()
	}
}

func int8BoundFromWit(b pg.Int8bound) Bound[int64] {
	switch b.Tag() {
	case pg.Int8boundIncluded:
		return Included(b.Included())
	case pg.Int8boundExcluded:
		return Excluded(b.Excluded())
	default:
		return Unbounded[int64]()
	}
}

func int8RangeFromWit(r pg.Int8range) Range[int64] {
	return Range[int64]{Start: int8BoundFromWit(r.Start), End: int8BoundFromWit(r.End)}
}

func numBoundToWit(b Bound[string]) pg.Numbound {
	switch b.Kind {
	case BoundIncluded:
		return pg.MakeNumboundIncluded(b.Value)
	case BoundExcluded:
		return pg.MakeNumboundExcluded(b.Value)
	default:
		return pg.MakeNumboundUnbounded()
	}
}

func numBoundFromWit(b pg.Numbound) Bound[string] {
	switch b.Tag() {
	case pg.NumboundIncluded:
		return Included(b.Included())
	case pg.NumboundExcluded:
		return Excluded(b.Excluded())
	default:
		return Unbounded[string]()
	}
}

func numRangeFromWit(r pg.Numrange) Range[string] {
	return Range[string]{Start: numBoundFromWit(r.Start), End: numBoundFromWit(r.End)}
}

func tsBoundToWit(b Bound[time.Time]) pg.Tsbound {
	switch b.Kind {
	case BoundIncluded:
		return pg.MakeTsboundIncluded(goTimeToTimestamp(b.Value))
	case BoundExcluded:
		return pg.MakeTsboundExcluded(goTimeToTimestamp(b.Value))
	default:
		return pg.MakeTsboundUnbounded()
	}
}

func tsBoundFromWit(b pg.Tsbound) Bound[time.Time] {
	switch b.Tag() {
	case pg.TsboundIncluded:
		return Included(timestampToGoTime(b.Included(), time.UTC))
	case pg.TsboundExcluded:
		return Excluded(timestampToGoTime(b.Excluded(), time.UTC))
	default:
		return Unbounded[time.Time]()
	}
}

func tsRangeFromWit(r pg.Tsrange) Range[time.Time] {
	return Range[time.Time]{Start: tsBoundFromWit(r.Start), End: tsBoundFromWit(r.End)}
}

func tstzBoundToWit(b Bound[time.Time]) pg.Tstzbound {
	switch b.Kind {
	case BoundIncluded:
		return pg.MakeTstzboundIncluded(goTimeToTimestamptz(b.Value))
	case BoundExcluded:
		return pg.MakeTstzboundExcluded(goTimeToTimestamptz(b.Value))
	default:
		return pg.MakeTstzboundUnbounded()
	}
}

func tstzBoundFromWit(b pg.Tstzbound) Bound[time.Time] {
	switch b.Tag() {
	case pg.TstzboundIncluded:
		return Included(timestamptzToGoTime(b.Included()))
	case pg.TstzboundExcluded:
		return Excluded(timestamptzToGoTime(b.Excluded()))
	default:
		return Unbounded[time.Time]()
	}
}

func tstzRangeFromWit(r pg.Tstzrange) Range[time.Time] {
	return Range[time.Time]{Start: tstzBoundFromWit(r.Start), End: tstzBoundFromWit(r.End)}
}

func dateBoundToWit(b Bound[time.Time]) pg.Datebound {
	switch b.Kind {
	case BoundIncluded:
		return pg.MakeDateboundIncluded(goTimeToDate(b.Value))
	case BoundExcluded:
		return pg.MakeDateboundExcluded(goTimeToDate(b.Value))
	default:
		return pg.MakeDateboundUnbounded()
	}
}

func dateBoundFromWit(b pg.Datebound) Bound[time.Time] {
	switch b.Tag() {
	case pg.DateboundIncluded:
		return Included(dateToGoTime(b.Included()))
	case pg.DateboundExcluded:
		return Excluded(dateToGoTime(b.Excluded()))
	default:
		return Unbounded[time.Time]()
	}
}

func dateRangeFromWit(r pg.Daterange) Range[time.Time] {
	return Range[time.Time]{Start: dateBoundFromWit(r.Start), End: dateBoundFromWit(r.End)}
}

// valueBoundToWit builds a host value-bound for a custom range, wrapping the
// bound value in a lazy resource (a host call).
func valueBoundToWit(b Bound[any]) (pg.ValueBound, error) {
	switch b.Kind {
	case BoundIncluded:
		v, err := encodeParam(b.Value)
		if err != nil {
			return pg.ValueBound{}, err
		}
		return pg.MakeValueBoundIncluded(pg.MakeLazyDbValue(v)), nil
	case BoundExcluded:
		v, err := encodeParam(b.Value)
		if err != nil {
			return pg.ValueBound{}, err
		}
		return pg.MakeValueBoundExcluded(pg.MakeLazyDbValue(v)), nil
	default:
		return pg.MakeValueBoundUnbounded(), nil
	}
}

// ── Rows / columns / result sets ─────────────────────────────────────────────

// Column describes a result column.
type Column struct {
	Ordinal    uint64
	Name       string
	DbTypeName string
}

func convColumns(cols []pg.DbColumn) []Column {
	out := make([]Column, len(cols))
	for i, c := range cols {
		out[i] = Column{Ordinal: c.Ordinal, Name: c.Name, DbTypeName: c.DbTypeName}
	}
	return out
}

// Row is a single result row, read positionally.
type Row struct{ values []pg.DbValue }

// Len returns the number of columns in the row.
func (r Row) Len() int { return len(r.values) }

func (r Row) at(i int) (pg.DbValue, error) {
	if i < 0 || i >= len(r.values) {
		return pg.DbValue{}, fmt.Errorf("golem/rdbms/postgres: column index %d out of range (%d columns)", i, len(r.values))
	}
	return r.values[i], nil
}

func typeErr(i int, v pg.DbValue, want string) error {
	return fmt.Errorf("golem/rdbms/postgres: column %d is %s, not %s", i, tagName(v.Tag()), want)
}

// IsNull reports whether column i is SQL NULL.
func (r Row) IsNull(i int) bool {
	v, err := r.at(i)
	return err == nil && v.Tag() == pg.DbValueNull
}

// Get decodes column i to a Go value: the int/float widths, string, bool,
// []byte, [golem.UUID], [time.Time] and the [types] structs for the families
// that map cleanly, nil for NULL, or an opaque [DbValue] for the composite
// families not yet decoded.
func (r Row) Get(i int) (any, error) {
	v, err := r.at(i)
	if err != nil {
		return nil, err
	}
	return decode(v), nil
}

// Int64 reads an integer column (any integer width, money or oid) as int64.
func (r Row) Int64(i int) (int64, error) {
	v, err := r.at(i)
	if err != nil {
		return 0, err
	}
	switch v.Tag() {
	case pg.DbValueInt8:
		return v.Int8(), nil
	case pg.DbValueInt4:
		return int64(v.Int4()), nil
	case pg.DbValueInt2:
		return int64(v.Int2()), nil
	case pg.DbValueCharacter:
		return int64(v.Character()), nil
	case pg.DbValueMoney:
		return v.Money(), nil
	case pg.DbValueOid:
		return int64(v.Oid()), nil
	default:
		return 0, typeErr(i, v, "int64")
	}
}

// Float64 reads a floating-point column as float64.
func (r Row) Float64(i int) (float64, error) {
	v, err := r.at(i)
	if err != nil {
		return 0, err
	}
	switch v.Tag() {
	case pg.DbValueFloat8:
		return v.Float8(), nil
	case pg.DbValueFloat4:
		return float64(v.Float4()), nil
	default:
		return 0, typeErr(i, v, "float64")
	}
}

// String reads a textual column (text/varchar/bpchar, numeric, json family or
// xml) as a string.
func (r Row) String(i int) (string, error) {
	v, err := r.at(i)
	if err != nil {
		return "", err
	}
	switch v.Tag() {
	case pg.DbValueText:
		return v.Text(), nil
	case pg.DbValueVarchar:
		return v.Varchar(), nil
	case pg.DbValueBpchar:
		return v.Bpchar(), nil
	case pg.DbValueNumeric:
		return v.Numeric(), nil
	case pg.DbValueJson:
		return v.Json(), nil
	case pg.DbValueJsonb:
		return v.Jsonb(), nil
	case pg.DbValueJsonpath:
		return v.Jsonpath(), nil
	case pg.DbValueXml:
		return v.Xml(), nil
	default:
		return "", typeErr(i, v, "string")
	}
}

// Bool reads a boolean column.
func (r Row) Bool(i int) (bool, error) {
	v, err := r.at(i)
	if err != nil {
		return false, err
	}
	if v.Tag() != pg.DbValueBoolean {
		return false, typeErr(i, v, "bool")
	}
	return v.Boolean(), nil
}

// Bytes reads a bytea column.
func (r Row) Bytes(i int) ([]byte, error) {
	v, err := r.at(i)
	if err != nil {
		return nil, err
	}
	if v.Tag() != pg.DbValueBytea {
		return nil, typeErr(i, v, "bytea")
	}
	return v.Bytea(), nil
}

// UUID reads a uuid column.
func (r Row) UUID(i int) (golem.UUID, error) {
	v, err := r.at(i)
	if err != nil {
		return golem.UUID{}, err
	}
	if v.Tag() != pg.DbValueUuid {
		return golem.UUID{}, typeErr(i, v, "uuid")
	}
	return uuidFromWit(v.Uuid()), nil
}

// Time reads a timestamp, timestamptz or date column as a [time.Time]
// (timestamps without a zone and dates are returned in UTC).
func (r Row) Time(i int) (time.Time, error) {
	v, err := r.at(i)
	if err != nil {
		return time.Time{}, err
	}
	switch v.Tag() {
	case pg.DbValueTimestamp:
		return timestampToGoTime(v.Timestamp(), time.UTC), nil
	case pg.DbValueTimestamptz:
		return timestamptzToGoTime(v.Timestamptz()), nil
	case pg.DbValueDate:
		return dateToGoTime(v.Date()), nil
	default:
		return time.Time{}, typeErr(i, v, "time.Time")
	}
}

// Typed getters for the exotic families. The flat ones (ranges, enum, sparse
// vector) read directly; the recursive ones (array, composite, domain, custom
// range) go through the host lazy resources, so — like [Row.Get] — they only run
// inside a query, not native tests.

// Int4Range reads an int4range column.
func (r Row) Int4Range(i int) (Range[int32], error) {
	v, err := r.at(i)
	if err != nil {
		return Range[int32]{}, err
	}
	if v.Tag() != pg.DbValueInt4range {
		return Range[int32]{}, typeErr(i, v, "int4range")
	}
	return int4RangeFromWit(v.Int4range()), nil
}

// Int8Range reads an int8range column.
func (r Row) Int8Range(i int) (Range[int64], error) {
	v, err := r.at(i)
	if err != nil {
		return Range[int64]{}, err
	}
	if v.Tag() != pg.DbValueInt8range {
		return Range[int64]{}, typeErr(i, v, "int8range")
	}
	return int8RangeFromWit(v.Int8range()), nil
}

// NumRange reads a numrange column (bounds as numeric strings).
func (r Row) NumRange(i int) (Range[string], error) {
	v, err := r.at(i)
	if err != nil {
		return Range[string]{}, err
	}
	if v.Tag() != pg.DbValueNumrange {
		return Range[string]{}, typeErr(i, v, "numrange")
	}
	return numRangeFromWit(v.Numrange()), nil
}

// TsRange reads a tsrange column (bounds in UTC).
func (r Row) TsRange(i int) (Range[time.Time], error) {
	v, err := r.at(i)
	if err != nil {
		return Range[time.Time]{}, err
	}
	if v.Tag() != pg.DbValueTsrange {
		return Range[time.Time]{}, typeErr(i, v, "tsrange")
	}
	return tsRangeFromWit(v.Tsrange()), nil
}

// TstzRange reads a tstzrange column (bound offsets preserved).
func (r Row) TstzRange(i int) (Range[time.Time], error) {
	v, err := r.at(i)
	if err != nil {
		return Range[time.Time]{}, err
	}
	if v.Tag() != pg.DbValueTstzrange {
		return Range[time.Time]{}, typeErr(i, v, "tstzrange")
	}
	return tstzRangeFromWit(v.Tstzrange()), nil
}

// DateRange reads a daterange column (bounds in UTC, date only).
func (r Row) DateRange(i int) (Range[time.Time], error) {
	v, err := r.at(i)
	if err != nil {
		return Range[time.Time]{}, err
	}
	if v.Tag() != pg.DbValueDaterange {
		return Range[time.Time]{}, typeErr(i, v, "daterange")
	}
	return dateRangeFromWit(v.Daterange()), nil
}

// Enum reads an enumeration column.
func (r Row) Enum(i int) (Enum, error) {
	v, err := r.at(i)
	if err != nil {
		return Enum{}, err
	}
	if v.Tag() != pg.DbValueEnumeration {
		return Enum{}, typeErr(i, v, "enumeration")
	}
	e := v.Enumeration()
	return Enum{Name: e.Name, Value: e.Value}, nil
}

// SparseVec reads a sparsevec column.
func (r Row) SparseVec(i int) (SparseVec, error) {
	v, err := r.at(i)
	if err != nil {
		return SparseVec{}, err
	}
	if v.Tag() != pg.DbValueSparsevec {
		return SparseVec{}, typeErr(i, v, "sparsevec")
	}
	s := v.Sparsevec()
	return SparseVec{Dim: int(s.Dim), Indices: s.Indices, Values: s.Values}, nil
}

// Array reads an array column into a slice, each element decoded like [Row.Get].
func (r Row) Array(i int) ([]any, error) {
	v, err := r.at(i)
	if err != nil {
		return nil, err
	}
	if v.Tag() != pg.DbValueArray {
		return nil, typeErr(i, v, "array")
	}
	return decodeLazy(v).([]any), nil
}

// Composite reads a composite column.
func (r Row) Composite(i int) (CompositeValue, error) {
	v, err := r.at(i)
	if err != nil {
		return CompositeValue{}, err
	}
	if v.Tag() != pg.DbValueComposite {
		return CompositeValue{}, typeErr(i, v, "composite")
	}
	return decodeLazy(v).(CompositeValue), nil
}

// Domain reads a domain column.
func (r Row) Domain(i int) (DomainValue, error) {
	v, err := r.at(i)
	if err != nil {
		return DomainValue{}, err
	}
	if v.Tag() != pg.DbValueDomain {
		return DomainValue{}, typeErr(i, v, "domain")
	}
	return decodeLazy(v).(DomainValue), nil
}

// Range reads a user-defined range column (use the typed range getters for the
// built-in range types).
func (r Row) Range(i int) (RangeValue, error) {
	v, err := r.at(i)
	if err != nil {
		return RangeValue{}, err
	}
	if v.Tag() != pg.DbValueRange {
		return RangeValue{}, typeErr(i, v, "range")
	}
	return decodeLazy(v).(RangeValue), nil
}

// Scan decodes the row into the given pointers, one per column. Supported
// destinations are *int64, *int, *float64, *string, *bool, *[]byte,
// *golem.UUID, *time.Time and *any (which receives whatever [Row.Get] returns).
func (r Row) Scan(dest ...any) error {
	if len(dest) != len(r.values) {
		return fmt.Errorf("golem/rdbms/postgres: Scan: %d destination(s) for %d column(s)", len(dest), len(r.values))
	}
	for i, d := range dest {
		if err := r.scanOne(i, d); err != nil {
			return err
		}
	}
	return nil
}

func (r Row) scanOne(i int, d any) error {
	switch p := d.(type) {
	case *any:
		v, err := r.Get(i)
		if err != nil {
			return err
		}
		*p = v
	case *int64:
		v, err := r.Int64(i)
		if err != nil {
			return err
		}
		*p = v
	case *int:
		v, err := r.Int64(i)
		if err != nil {
			return err
		}
		*p = int(v)
	case *float64:
		v, err := r.Float64(i)
		if err != nil {
			return err
		}
		*p = v
	case *string:
		v, err := r.String(i)
		if err != nil {
			return err
		}
		*p = v
	case *bool:
		v, err := r.Bool(i)
		if err != nil {
			return err
		}
		*p = v
	case *[]byte:
		v, err := r.Bytes(i)
		if err != nil {
			return err
		}
		*p = v
	case *golem.UUID:
		v, err := r.UUID(i)
		if err != nil {
			return err
		}
		*p = v
	case *time.Time:
		v, err := r.Time(i)
		if err != nil {
			return err
		}
		*p = v
	default:
		return fmt.Errorf("golem/rdbms/postgres: Scan: unsupported destination type %T for column %d", d, i)
	}
	return nil
}

// ResultSet is the eager result of a [DB.Query] / [Tx.Query]: all columns and
// rows, already fetched.
type ResultSet struct {
	columns []Column
	rows    []Row
}

func newResultSet(res pg.DbResult) *ResultSet {
	rows := make([]Row, len(res.Rows))
	for i, r := range res.Rows {
		rows[i] = Row{values: r.Values}
	}
	return &ResultSet{columns: convColumns(res.Columns), rows: rows}
}

// Columns returns the result column metadata.
func (rs *ResultSet) Columns() []Column { return rs.columns }

// Rows returns the result rows.
func (rs *ResultSet) Rows() []Row { return rs.rows }

// ── Streaming results ─────────────────────────────────────────────────────────

// Stream is a cursor over a [DB.QueryStream] / [Tx.QueryStream] result. Iterate
// with [Stream.Next] until it reports no more rows, then call [Stream.Close].
type Stream struct {
	raw  *pg.DbResultStream
	cols []Column
	buf  []Row
	idx  int
	done bool
}

// Columns returns the result column metadata.
func (s *Stream) Columns() []Column {
	if s.cols == nil {
		s.cols = convColumns(s.raw.GetColumns())
	}
	return s.cols
}

// Next returns the next row. ok is false (with a nil error) once the stream is
// exhausted.
func (s *Stream) Next() (row Row, ok bool, err error) {
	for s.idx >= len(s.buf) {
		if s.done {
			return Row{}, false, nil
		}
		opt := s.raw.GetNext()
		if opt.IsNone() {
			s.done = true
			return Row{}, false, nil
		}
		batch := opt.Some()
		s.buf = make([]Row, len(batch))
		for i, r := range batch {
			s.buf[i] = Row{values: r.Values}
		}
		s.idx = 0
	}
	row = s.buf[s.idx]
	s.idx++
	return row, true, nil
}

// Close releases the stream.
func (s *Stream) Close() { s.raw.Drop() }

// ── Connection ────────────────────────────────────────────────────────────────

// DB is an open Postgres connection.
type DB struct{ raw *pg.DbConnection }

// Open opens a connection to the given Postgres URL
// ("postgres://user:pass@host:port/database").
func Open(address string) (*DB, error) {
	r := pg.DbConnectionOpen(address)
	if r.IsErr() {
		return nil, pgError(r.Err())
	}
	return &DB{raw: r.Ok()}, nil
}

// Query runs a statement that returns rows.
func (db *DB) Query(sql string, args ...any) (*ResultSet, error) {
	params, err := encodeParams(args)
	if err != nil {
		return nil, err
	}
	r := db.raw.Query(sql, params)
	if r.IsErr() {
		return nil, pgError(r.Err())
	}
	return newResultSet(r.Ok()), nil
}

// Exec runs a statement that does not return rows, returning the number of
// affected rows.
func (db *DB) Exec(sql string, args ...any) (uint64, error) {
	params, err := encodeParams(args)
	if err != nil {
		return 0, err
	}
	r := db.raw.Execute(sql, params)
	if r.IsErr() {
		return 0, pgError(r.Err())
	}
	return r.Ok(), nil
}

// QueryStream runs a row-returning statement and streams the results.
func (db *DB) QueryStream(sql string, args ...any) (*Stream, error) {
	params, err := encodeParams(args)
	if err != nil {
		return nil, err
	}
	r := db.raw.QueryStream(sql, params)
	if r.IsErr() {
		return nil, pgError(r.Err())
	}
	return &Stream{raw: r.Ok()}, nil
}

// Begin starts a transaction.
func (db *DB) Begin() (*Tx, error) {
	r := db.raw.BeginTransaction()
	if r.IsErr() {
		return nil, pgError(r.Err())
	}
	return &Tx{raw: r.Ok()}, nil
}

// Transaction runs fn inside a transaction, committing if fn returns nil and
// rolling back (and returning fn's error) otherwise.
func (db *DB) Transaction(fn func(*Tx) error) error {
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	if err := fn(tx); err != nil {
		_ = tx.Rollback()
		return err
	}
	return tx.Commit()
}

// Close releases the connection.
func (db *DB) Close() { db.raw.Drop() }

// ── Transactions ──────────────────────────────────────────────────────────────

// Tx is an in-progress transaction. Finish it with [Tx.Commit] or [Tx.Rollback].
type Tx struct{ raw *pg.DbTransaction }

// Query runs a row-returning statement in the transaction.
func (tx *Tx) Query(sql string, args ...any) (*ResultSet, error) {
	params, err := encodeParams(args)
	if err != nil {
		return nil, err
	}
	r := tx.raw.Query(sql, params)
	if r.IsErr() {
		return nil, pgError(r.Err())
	}
	return newResultSet(r.Ok()), nil
}

// Exec runs a non-row-returning statement in the transaction, returning the
// number of affected rows.
func (tx *Tx) Exec(sql string, args ...any) (uint64, error) {
	params, err := encodeParams(args)
	if err != nil {
		return 0, err
	}
	r := tx.raw.Execute(sql, params)
	if r.IsErr() {
		return 0, pgError(r.Err())
	}
	return r.Ok(), nil
}

// QueryStream runs a row-returning statement in the transaction and streams the
// results.
func (tx *Tx) QueryStream(sql string, args ...any) (*Stream, error) {
	params, err := encodeParams(args)
	if err != nil {
		return nil, err
	}
	r := tx.raw.QueryStream(sql, params)
	if r.IsErr() {
		return nil, pgError(r.Err())
	}
	return &Stream{raw: r.Ok()}, nil
}

// Commit commits the transaction.
func (tx *Tx) Commit() error {
	r := tx.raw.Commit()
	tx.raw.Drop()
	if r.IsErr() {
		return pgError(r.Err())
	}
	return nil
}

// Rollback aborts the transaction.
func (tx *Tx) Rollback() error {
	r := tx.raw.Rollback()
	tx.raw.Drop()
	if r.IsErr() {
		return pgError(r.Err())
	}
	return nil
}
