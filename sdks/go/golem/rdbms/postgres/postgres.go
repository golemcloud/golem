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
// The recursive/composite value families (arrays, composites, domains, ranges)
// are surfaced from queries as an opaque [DbValue] for now; first-class support
// is a planned follow-up. Everything else — including json, uuid, temporal,
// inet/cidr/macaddr, bit strings and vectors — maps to a Go value.
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

// ── DbValue (typed parameters + opaque escape hatch) ─────────────────────────

// DbValue is a Postgres value whose exact column type is chosen explicitly,
// rather than inferred from a Go value. Build one with a constructor in this
// package ([Numeric], [JSONB], [Int4], …) and pass it as a query argument. A
// value read back from a column whose family this wrapper does not yet decode
// (an array, composite, domain or range) is also returned as a DbValue; inspect
// its family with [DbValue.Kind].
type DbValue struct{ raw pg.DbValue }

// tagName gives a db-value family its display name (used by [DbValue.Kind] and
// error text — never by encode/decode). It switches on the generated constants,
// not on the raw tag position, so a reordered or inserted WIT case still names
// correctly and a renamed or removed one fails to compile here. A newly appended
// WIT case shows as "unknown(N)" and is handled as an opaque [DbValue] (see the
// default in decodeValue) until a case and name are added below.
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

// Kind returns the name of the value's Postgres type family (for example "array"
// or "numeric").
func (v DbValue) Kind() string { return tagName(v.raw.Tag()) }

// IsNull reports whether the value is SQL NULL.
func (v DbValue) IsNull() bool { return v.raw.Tag() == pg.DbValueNull }

func (v DbValue) String() string { return "postgres:" + v.Kind() }

// Null builds a SQL NULL parameter.
func Null() DbValue { return DbValue{pg.MakeDbValueNull()} }

// Char builds a Postgres "char" (single-byte integer) parameter.
func Char(v int8) DbValue { return DbValue{pg.MakeDbValueCharacter(v)} }

// Int2 builds a smallint parameter.
func Int2(v int16) DbValue { return DbValue{pg.MakeDbValueInt2(v)} }

// Int4 builds an integer parameter.
func Int4(v int32) DbValue { return DbValue{pg.MakeDbValueInt4(v)} }

// Int8 builds a bigint parameter.
func Int8(v int64) DbValue { return DbValue{pg.MakeDbValueInt8(v)} }

// Float4 builds a real parameter.
func Float4(v float32) DbValue { return DbValue{pg.MakeDbValueFloat4(v)} }

// Float8 builds a double precision parameter.
func Float8(v float64) DbValue { return DbValue{pg.MakeDbValueFloat8(v)} }

// Numeric builds an exact numeric/decimal parameter from its string form (so no
// precision is lost).
func Numeric(v string) DbValue { return DbValue{pg.MakeDbValueNumeric(v)} }

// Money builds a money parameter (value in the smallest currency unit).
func Money(v int64) DbValue { return DbValue{pg.MakeDbValueMoney(v)} }

// Oid builds an object identifier parameter.
func Oid(v uint32) DbValue { return DbValue{pg.MakeDbValueOid(v)} }

// Varchar builds a varchar parameter.
func Varchar(v string) DbValue { return DbValue{pg.MakeDbValueVarchar(v)} }

// Bpchar builds a blank-padded char parameter.
func Bpchar(v string) DbValue { return DbValue{pg.MakeDbValueBpchar(v)} }

// JSON builds a json parameter from its serialized text.
func JSON(v string) DbValue { return DbValue{pg.MakeDbValueJson(v)} }

// JSONB builds a jsonb parameter from its serialized text.
func JSONB(v string) DbValue { return DbValue{pg.MakeDbValueJsonb(v)} }

// JSONPath builds a jsonpath parameter.
func JSONPath(v string) DbValue { return DbValue{pg.MakeDbValueJsonpath(v)} }

// XML builds an xml parameter.
func XML(v string) DbValue { return DbValue{pg.MakeDbValueXml(v)} }

// Bit builds a fixed-length bit-string parameter.
func Bit(v []bool) DbValue { return DbValue{pg.MakeDbValueBit(v)} }

// Varbit builds a varying bit-string parameter.
func Varbit(v []bool) DbValue { return DbValue{pg.MakeDbValueVarbit(v)} }

// Inet builds an inet (host address) parameter.
func Inet(a netip.Addr) DbValue { return DbValue{pg.MakeDbValueInet(ipToWit(a))} }

// Cidr builds a cidr (network address) parameter.
func Cidr(a netip.Addr) DbValue { return DbValue{pg.MakeDbValueCidr(ipToWit(a))} }

// Macaddr builds a macaddr parameter.
func Macaddr(m types.MacAddr) DbValue { return DbValue{pg.MakeDbValueMacaddr(macToWit(m))} }

// Interval builds an interval parameter.
func Interval(iv types.Interval) DbValue {
	return DbValue{pg.MakeDbValueInterval(pg.Interval{
		Months: int32(iv.Months), Days: int32(iv.Days), Microseconds: iv.Microseconds,
	})}
}

// Date builds a date parameter.
func Date(d types.Date) DbValue {
	return DbValue{pg.MakeDbValueDate(pg.Date{Year: int32(d.Year), Month: uint8(d.Month), Day: uint8(d.Day)})}
}

// Time builds a time-of-day parameter.
func Time(t types.Time) DbValue { return DbValue{pg.MakeDbValueTime(typesTimeToWit(t))} }

// Timetz builds a time-of-day-with-offset parameter.
func Timetz(t types.Timetz) DbValue {
	return DbValue{pg.MakeDbValueTimetz(pg.Timetz{Time: typesTimeToWit(t.Time), Offset: int32(t.OffsetSeconds)})}
}

// Timestamp builds a timestamp (no time zone) parameter.
func Timestamp(ts types.Timestamp) DbValue {
	return DbValue{pg.MakeDbValueTimestamp(typesTimestampToWit(ts))}
}

// Timestamptz builds a timestamp-with-time-zone parameter.
func Timestamptz(ts types.Timestamptz) DbValue {
	return DbValue{pg.MakeDbValueTimestamptz(pg.Timestamptz{
		Timestamp: typesTimestampToWit(ts.Timestamp), Offset: int32(ts.OffsetSeconds),
	})}
}

// Vector builds a pgvector vector parameter.
func Vector(v []float32) DbValue { return DbValue{pg.MakeDbValueVector(v)} }

// Halfvec builds a pgvector halfvec parameter.
func Halfvec(v []float32) DbValue { return DbValue{pg.MakeDbValueHalfvec(v)} }

// ── Parameter encoding ───────────────────────────────────────────────────────

func encodeParam(v any) (pg.DbValue, error) {
	switch x := v.(type) {
	case nil:
		return pg.MakeDbValueNull(), nil
	case DbValue:
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

func decodeValue(v pg.DbValue) any {
	switch v.Tag() {
	case pg.DbValueNull:
		return nil
	case pg.DbValueCharacter:
		return v.Character()
	case pg.DbValueInt2:
		return v.Int2()
	case pg.DbValueInt4:
		return v.Int4()
	case pg.DbValueInt8:
		return v.Int8()
	case pg.DbValueFloat4:
		return v.Float4()
	case pg.DbValueFloat8:
		return v.Float8()
	case pg.DbValueNumeric:
		return v.Numeric()
	case pg.DbValueBoolean:
		return v.Boolean()
	case pg.DbValueText:
		return v.Text()
	case pg.DbValueVarchar:
		return v.Varchar()
	case pg.DbValueBpchar:
		return v.Bpchar()
	case pg.DbValueTimestamp:
		return timestampToGoTime(v.Timestamp(), time.UTC)
	case pg.DbValueTimestamptz:
		return timestamptzToGoTime(v.Timestamptz())
	case pg.DbValueDate:
		return dateToGoTime(v.Date())
	case pg.DbValueTime:
		return witTimeToTypes(v.Time())
	case pg.DbValueTimetz:
		return types.Timetz{Time: witTimeToTypes(v.Timetz().Time), OffsetSeconds: int(v.Timetz().Offset)}
	case pg.DbValueInterval:
		iv := v.Interval()
		return types.Interval{Months: int(iv.Months), Days: int(iv.Days), Microseconds: iv.Microseconds}
	case pg.DbValueBytea:
		return v.Bytea()
	case pg.DbValueJson:
		return v.Json()
	case pg.DbValueJsonb:
		return v.Jsonb()
	case pg.DbValueJsonpath:
		return v.Jsonpath()
	case pg.DbValueXml:
		return v.Xml()
	case pg.DbValueUuid:
		return uuidFromWit(v.Uuid())
	case pg.DbValueInet:
		return ipFromWit(v.Inet())
	case pg.DbValueCidr:
		return ipFromWit(v.Cidr())
	case pg.DbValueMacaddr:
		return macFromWit(v.Macaddr())
	case pg.DbValueBit:
		return v.Bit()
	case pg.DbValueVarbit:
		return v.Varbit()
	case pg.DbValueMoney:
		return v.Money()
	case pg.DbValueOid:
		return v.Oid()
	case pg.DbValueVector:
		return v.Vector()
	case pg.DbValueHalfvec:
		return v.Halfvec()
	default:
		// enumeration, composite, domain, array, range, sparsevec — opaque for now.
		return DbValue{raw: v}
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
	return decodeValue(v), nil
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
