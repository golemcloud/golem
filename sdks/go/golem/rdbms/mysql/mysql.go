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

// Package mysql is a Go wrapper over Golem's durable MySQL client (golem:rdbms).
// It mirrors the postgres package — open a connection with a URL, then run
// parametrised statements in a database/sql-flavoured style:
//
//	db, err := mysql.Open("mysql://user:pass@host:3306/app")
//	rs, err := db.Query("SELECT id, name FROM users WHERE active = ?", true)
//	for _, row := range rs.Rows() {
//	    var id int64
//	    var name string
//	    row.Scan(&id, &name)
//	}
//
// Parameters are ordinary Go values: nil, bool, the signed and unsigned int
// widths, float32/float64, string, []byte and [time.Time] map to the natural
// MySQL types. Where the exact column type matters (decimal, a specific integer
// width, json, enum) build the value with a constructor in this package
// ([Decimal], [Int], [JSON], …), which returns a [DbValue] you pass like any
// other argument. Columns are read positionally with the typed getters on [Row]
// ([Row.Int64], [Row.String], …), the generic [Row.Get], or [Row.Scan].
//
// Like the other WASI wrappers, operations return an error, distinct from the
// fail-loud control-flow surface. The connection is durable — every
// query/execute/commit is journaled and replayed — and because these are remote
// side effects, using it inside a read-only method traps. Unlike postgres, MySQL
// has no recursive value families, so every value maps to a Go value.
//
// Pair a fallible call with golem.Must / golem.Must0 / golem.Must2 to abort the
// invocation on error.
package mysql

import (
	"fmt"
	"time"

	my "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_rdbms_mysql"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/types"
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

// Error is a MySQL host error.
type Error struct {
	Kind    ErrorKind
	Message string
}

func (e *Error) Error() string { return "golem/rdbms/mysql: " + e.Message }

func myError(e my.Error) error {
	switch e.Tag() {
	case my.ErrorConnectionFailure:
		return &Error{Kind: ConnectionFailure, Message: e.ConnectionFailure()}
	case my.ErrorQueryParameterFailure:
		return &Error{Kind: QueryParameterFailure, Message: e.QueryParameterFailure()}
	case my.ErrorQueryExecutionFailure:
		return &Error{Kind: QueryExecutionFailure, Message: e.QueryExecutionFailure()}
	case my.ErrorQueryResponseFailure:
		return &Error{Kind: QueryResponseFailure, Message: e.QueryResponseFailure()}
	default:
		return &Error{Kind: Other, Message: e.Other()}
	}
}

// ── DbValue (typed parameters + opaque escape hatch) ─────────────────────────

// DbValue is a MySQL value whose exact column type is chosen explicitly, rather
// than inferred from a Go value. Build one with a constructor in this package
// ([Decimal], [Int], [JSON], …) and pass it as a query argument. A value read
// back from a column family this wrapper does not recognize is also returned as
// a DbValue; inspect its family with [DbValue.Kind].
type DbValue struct{ raw my.DbValue }

// tagName gives a db-value family its display name (used by [DbValue.Kind] and
// error text — never by encode/decode). It switches on the generated constants,
// not on the raw tag position, so a reordered or inserted WIT case still names
// correctly and a renamed or removed one fails to compile here. A newly appended
// WIT case shows as "unknown(N)" and is handled as an opaque [DbValue] (see the
// default in decodeValue) until a case and name are added below.
func tagName(tag uint8) string {
	switch tag {
	case my.DbValueBoolean:
		return "boolean"
	case my.DbValueTinyint:
		return "tinyint"
	case my.DbValueSmallint:
		return "smallint"
	case my.DbValueMediumint:
		return "mediumint"
	case my.DbValueInt:
		return "int"
	case my.DbValueBigint:
		return "bigint"
	case my.DbValueTinyintUnsigned:
		return "tinyint-unsigned"
	case my.DbValueSmallintUnsigned:
		return "smallint-unsigned"
	case my.DbValueMediumintUnsigned:
		return "mediumint-unsigned"
	case my.DbValueIntUnsigned:
		return "int-unsigned"
	case my.DbValueBigintUnsigned:
		return "bigint-unsigned"
	case my.DbValueFloat:
		return "float"
	case my.DbValueDouble:
		return "double"
	case my.DbValueDecimal:
		return "decimal"
	case my.DbValueDate:
		return "date"
	case my.DbValueDatetime:
		return "datetime"
	case my.DbValueTimestamp:
		return "timestamp"
	case my.DbValueTime:
		return "time"
	case my.DbValueYear:
		return "year"
	case my.DbValueFixchar:
		return "fixchar"
	case my.DbValueVarchar:
		return "varchar"
	case my.DbValueTinytext:
		return "tinytext"
	case my.DbValueText:
		return "text"
	case my.DbValueMediumtext:
		return "mediumtext"
	case my.DbValueLongtext:
		return "longtext"
	case my.DbValueBinary:
		return "binary"
	case my.DbValueVarbinary:
		return "varbinary"
	case my.DbValueTinyblob:
		return "tinyblob"
	case my.DbValueBlob:
		return "blob"
	case my.DbValueMediumblob:
		return "mediumblob"
	case my.DbValueLongblob:
		return "longblob"
	case my.DbValueEnumeration:
		return "enumeration"
	case my.DbValueSet:
		return "set"
	case my.DbValueBit:
		return "bit"
	case my.DbValueJson:
		return "json"
	case my.DbValueNull:
		return "null"
	default:
		return fmt.Sprintf("unknown(%d)", tag)
	}
}

// Kind returns the name of the value's MySQL type family (for example "decimal"
// or "bigint-unsigned").
func (v DbValue) Kind() string { return tagName(v.raw.Tag()) }

// IsNull reports whether the value is SQL NULL.
func (v DbValue) IsNull() bool { return v.raw.Tag() == my.DbValueNull }

func (v DbValue) String() string { return "mysql:" + v.Kind() }

// Null builds a SQL NULL parameter.
func Null() DbValue { return DbValue{my.MakeDbValueNull()} }

// Boolean builds a boolean parameter.
func Boolean(v bool) DbValue { return DbValue{my.MakeDbValueBoolean(v)} }

// Tinyint builds a signed tinyint parameter.
func Tinyint(v int8) DbValue { return DbValue{my.MakeDbValueTinyint(v)} }

// Smallint builds a signed smallint parameter.
func Smallint(v int16) DbValue { return DbValue{my.MakeDbValueSmallint(v)} }

// Mediumint builds a signed mediumint parameter.
func Mediumint(v int32) DbValue { return DbValue{my.MakeDbValueMediumint(v)} }

// Int builds a signed int parameter.
func Int(v int32) DbValue { return DbValue{my.MakeDbValueInt(v)} }

// Bigint builds a signed bigint parameter.
func Bigint(v int64) DbValue { return DbValue{my.MakeDbValueBigint(v)} }

// TinyintUnsigned builds an unsigned tinyint parameter.
func TinyintUnsigned(v uint8) DbValue { return DbValue{my.MakeDbValueTinyintUnsigned(v)} }

// SmallintUnsigned builds an unsigned smallint parameter.
func SmallintUnsigned(v uint16) DbValue { return DbValue{my.MakeDbValueSmallintUnsigned(v)} }

// MediumintUnsigned builds an unsigned mediumint parameter.
func MediumintUnsigned(v uint32) DbValue { return DbValue{my.MakeDbValueMediumintUnsigned(v)} }

// IntUnsigned builds an unsigned int parameter.
func IntUnsigned(v uint32) DbValue { return DbValue{my.MakeDbValueIntUnsigned(v)} }

// BigintUnsigned builds an unsigned bigint parameter.
func BigintUnsigned(v uint64) DbValue { return DbValue{my.MakeDbValueBigintUnsigned(v)} }

// Float builds a single-precision float parameter.
func Float(v float32) DbValue { return DbValue{my.MakeDbValueFloat(v)} }

// Double builds a double-precision float parameter.
func Double(v float64) DbValue { return DbValue{my.MakeDbValueDouble(v)} }

// Decimal builds an exact decimal parameter from its string form (so no
// precision is lost).
func Decimal(v string) DbValue { return DbValue{my.MakeDbValueDecimal(v)} }

// Year builds a year parameter.
func Year(v uint16) DbValue { return DbValue{my.MakeDbValueYear(v)} }

// Fixchar builds a fixed-length char parameter.
func Fixchar(v string) DbValue { return DbValue{my.MakeDbValueFixchar(v)} }

// Varchar builds a varchar parameter.
func Varchar(v string) DbValue { return DbValue{my.MakeDbValueVarchar(v)} }

// Tinytext builds a tinytext parameter.
func Tinytext(v string) DbValue { return DbValue{my.MakeDbValueTinytext(v)} }

// Text builds a text parameter.
func Text(v string) DbValue { return DbValue{my.MakeDbValueText(v)} }

// Mediumtext builds a mediumtext parameter.
func Mediumtext(v string) DbValue { return DbValue{my.MakeDbValueMediumtext(v)} }

// Longtext builds a longtext parameter.
func Longtext(v string) DbValue { return DbValue{my.MakeDbValueLongtext(v)} }

// Binary builds a fixed-length binary parameter.
func Binary(v []byte) DbValue { return DbValue{my.MakeDbValueBinary(v)} }

// Varbinary builds a varbinary parameter.
func Varbinary(v []byte) DbValue { return DbValue{my.MakeDbValueVarbinary(v)} }

// Tinyblob builds a tinyblob parameter.
func Tinyblob(v []byte) DbValue { return DbValue{my.MakeDbValueTinyblob(v)} }

// Blob builds a blob parameter.
func Blob(v []byte) DbValue { return DbValue{my.MakeDbValueBlob(v)} }

// Mediumblob builds a mediumblob parameter.
func Mediumblob(v []byte) DbValue { return DbValue{my.MakeDbValueMediumblob(v)} }

// Longblob builds a longblob parameter.
func Longblob(v []byte) DbValue { return DbValue{my.MakeDbValueLongblob(v)} }

// Enumeration builds an enum parameter (the selected label).
func Enumeration(v string) DbValue { return DbValue{my.MakeDbValueEnumeration(v)} }

// Set builds a set parameter (the comma-separated labels).
func Set(v string) DbValue { return DbValue{my.MakeDbValueSet(v)} }

// Bit builds a bit-string parameter.
func Bit(v []bool) DbValue { return DbValue{my.MakeDbValueBit(v)} }

// JSON builds a json parameter from its serialized text.
func JSON(v string) DbValue { return DbValue{my.MakeDbValueJson(v)} }

// Date builds a date parameter.
func Date(d types.Date) DbValue {
	return DbValue{my.MakeDbValueDate(my.Date{Year: int32(d.Year), Month: uint8(d.Month), Day: uint8(d.Day)})}
}

// Datetime builds a datetime parameter.
func Datetime(ts types.Timestamp) DbValue {
	return DbValue{my.MakeDbValueDatetime(typesTimestampToWit(ts))}
}

// Timestamp builds a timestamp parameter.
func Timestamp(ts types.Timestamp) DbValue {
	return DbValue{my.MakeDbValueTimestamp(typesTimestampToWit(ts))}
}

// Time builds a time-of-day parameter.
func Time(t types.Time) DbValue { return DbValue{my.MakeDbValueTime(typesTimeToWit(t))} }

// ── Parameter encoding ───────────────────────────────────────────────────────

func encodeParam(v any) (my.DbValue, error) {
	switch x := v.(type) {
	case nil:
		return my.MakeDbValueNull(), nil
	case DbValue:
		return x.raw, nil
	case bool:
		return my.MakeDbValueBoolean(x), nil
	case int:
		return my.MakeDbValueBigint(int64(x)), nil
	case int64:
		return my.MakeDbValueBigint(x), nil
	case int32:
		return my.MakeDbValueInt(x), nil
	case int16:
		return my.MakeDbValueSmallint(x), nil
	case int8:
		return my.MakeDbValueTinyint(x), nil
	case uint:
		return my.MakeDbValueBigintUnsigned(uint64(x)), nil
	case uint64:
		return my.MakeDbValueBigintUnsigned(x), nil
	case uint32:
		return my.MakeDbValueIntUnsigned(x), nil
	case uint16:
		return my.MakeDbValueSmallintUnsigned(x), nil
	case uint8:
		return my.MakeDbValueTinyintUnsigned(x), nil
	case float64:
		return my.MakeDbValueDouble(x), nil
	case float32:
		return my.MakeDbValueFloat(x), nil
	case string:
		return my.MakeDbValueVarchar(x), nil
	case []byte:
		return my.MakeDbValueBlob(x), nil
	case time.Time:
		return my.MakeDbValueDatetime(goTimeToTimestamp(x)), nil
	default:
		return my.DbValue{}, fmt.Errorf("unsupported parameter type %T; wrap it with a mysql.* constructor", v)
	}
}

func encodeParams(args []any) ([]my.DbValue, error) {
	if len(args) == 0 {
		return nil, nil
	}
	out := make([]my.DbValue, len(args))
	for i, a := range args {
		v, err := encodeParam(a)
		if err != nil {
			return nil, fmt.Errorf("golem/rdbms/mysql: parameter %d: %w", i+1, err)
		}
		out[i] = v
	}
	return out, nil
}

// ── Value decoding ────────────────────────────────────────────────────────────

func decodeValue(v my.DbValue) any {
	switch v.Tag() {
	case my.DbValueNull:
		return nil
	case my.DbValueBoolean:
		return v.Boolean()
	case my.DbValueTinyint:
		return v.Tinyint()
	case my.DbValueSmallint:
		return v.Smallint()
	case my.DbValueMediumint:
		return v.Mediumint()
	case my.DbValueInt:
		return v.Int()
	case my.DbValueBigint:
		return v.Bigint()
	case my.DbValueTinyintUnsigned:
		return v.TinyintUnsigned()
	case my.DbValueSmallintUnsigned:
		return v.SmallintUnsigned()
	case my.DbValueMediumintUnsigned:
		return v.MediumintUnsigned()
	case my.DbValueIntUnsigned:
		return v.IntUnsigned()
	case my.DbValueBigintUnsigned:
		return v.BigintUnsigned()
	case my.DbValueFloat:
		return v.Float()
	case my.DbValueDouble:
		return v.Double()
	case my.DbValueDecimal:
		return v.Decimal()
	case my.DbValueDate:
		return dateToGoTime(v.Date())
	case my.DbValueDatetime:
		return timestampToGoTime(v.Datetime())
	case my.DbValueTimestamp:
		return timestampToGoTime(v.Timestamp())
	case my.DbValueTime:
		return witTimeToTypes(v.Time())
	case my.DbValueYear:
		return v.Year()
	case my.DbValueFixchar:
		return v.Fixchar()
	case my.DbValueVarchar:
		return v.Varchar()
	case my.DbValueTinytext:
		return v.Tinytext()
	case my.DbValueText:
		return v.Text()
	case my.DbValueMediumtext:
		return v.Mediumtext()
	case my.DbValueLongtext:
		return v.Longtext()
	case my.DbValueBinary:
		return v.Binary()
	case my.DbValueVarbinary:
		return v.Varbinary()
	case my.DbValueTinyblob:
		return v.Tinyblob()
	case my.DbValueBlob:
		return v.Blob()
	case my.DbValueMediumblob:
		return v.Mediumblob()
	case my.DbValueLongblob:
		return v.Longblob()
	case my.DbValueEnumeration:
		return v.Enumeration()
	case my.DbValueSet:
		return v.Set()
	case my.DbValueBit:
		return v.Bit()
	case my.DbValueJson:
		return v.Json()
	default:
		return DbValue{raw: v}
	}
}

// ── Temporal conversions ──────────────────────────────────────────────────────

func goTimeToTimestamp(t time.Time) my.Timestamp {
	return my.Timestamp{
		Date: my.Date{Year: int32(t.Year()), Month: uint8(t.Month()), Day: uint8(t.Day())},
		Time: my.Time{Hour: uint8(t.Hour()), Minute: uint8(t.Minute()), Second: uint8(t.Second()), Nanosecond: uint32(t.Nanosecond())},
	}
}

func timestampToGoTime(ts my.Timestamp) time.Time {
	return time.Date(int(ts.Date.Year), time.Month(ts.Date.Month), int(ts.Date.Day),
		int(ts.Time.Hour), int(ts.Time.Minute), int(ts.Time.Second), int(ts.Time.Nanosecond), time.UTC)
}

func dateToGoTime(d my.Date) time.Time {
	return time.Date(int(d.Year), time.Month(d.Month), int(d.Day), 0, 0, 0, 0, time.UTC)
}

func witTimeToTypes(t my.Time) types.Time {
	return types.Time{Hour: int(t.Hour), Minute: int(t.Minute), Second: int(t.Second), Nanosecond: int(t.Nanosecond)}
}

func typesTimeToWit(t types.Time) my.Time {
	return my.Time{Hour: uint8(t.Hour), Minute: uint8(t.Minute), Second: uint8(t.Second), Nanosecond: uint32(t.Nanosecond)}
}

func typesTimestampToWit(ts types.Timestamp) my.Timestamp {
	return my.Timestamp{
		Date: my.Date{Year: int32(ts.Date.Year), Month: uint8(ts.Date.Month), Day: uint8(ts.Date.Day)},
		Time: typesTimeToWit(ts.Time),
	}
}

// ── Rows / columns / result sets ─────────────────────────────────────────────

// Column describes a result column.
type Column struct {
	Ordinal    uint64
	Name       string
	DbTypeName string
}

func convColumns(cols []my.DbColumn) []Column {
	out := make([]Column, len(cols))
	for i, c := range cols {
		out[i] = Column{Ordinal: c.Ordinal, Name: c.Name, DbTypeName: c.DbTypeName}
	}
	return out
}

// Row is a single result row, read positionally.
type Row struct{ values []my.DbValue }

// Len returns the number of columns in the row.
func (r Row) Len() int { return len(r.values) }

func (r Row) at(i int) (my.DbValue, error) {
	if i < 0 || i >= len(r.values) {
		return my.DbValue{}, fmt.Errorf("golem/rdbms/mysql: column index %d out of range (%d columns)", i, len(r.values))
	}
	return r.values[i], nil
}

func typeErr(i int, v my.DbValue, want string) error {
	return fmt.Errorf("golem/rdbms/mysql: column %d is %s, not %s", i, tagName(v.Tag()), want)
}

// IsNull reports whether column i is SQL NULL.
func (r Row) IsNull(i int) bool {
	v, err := r.at(i)
	return err == nil && v.Tag() == my.DbValueNull
}

// Get decodes column i to a Go value: bool, the signed/unsigned int widths,
// float32/float64, string, []byte, [time.Time] and the [types] structs for the
// families that map cleanly, nil for NULL, or an opaque [DbValue] otherwise.
func (r Row) Get(i int) (any, error) {
	v, err := r.at(i)
	if err != nil {
		return nil, err
	}
	return decodeValue(v), nil
}

// Int64 reads an integer column (any signed or unsigned width, or year) as int64.
// A bigint-unsigned above math.MaxInt64 wraps; use [Row.Uint64] for those.
func (r Row) Int64(i int) (int64, error) {
	v, err := r.at(i)
	if err != nil {
		return 0, err
	}
	switch v.Tag() {
	case my.DbValueTinyint:
		return int64(v.Tinyint()), nil
	case my.DbValueSmallint:
		return int64(v.Smallint()), nil
	case my.DbValueMediumint:
		return int64(v.Mediumint()), nil
	case my.DbValueInt:
		return int64(v.Int()), nil
	case my.DbValueBigint:
		return v.Bigint(), nil
	case my.DbValueTinyintUnsigned:
		return int64(v.TinyintUnsigned()), nil
	case my.DbValueSmallintUnsigned:
		return int64(v.SmallintUnsigned()), nil
	case my.DbValueMediumintUnsigned:
		return int64(v.MediumintUnsigned()), nil
	case my.DbValueIntUnsigned:
		return int64(v.IntUnsigned()), nil
	case my.DbValueBigintUnsigned:
		return int64(v.BigintUnsigned()), nil
	case my.DbValueYear:
		return int64(v.Year()), nil
	default:
		return 0, typeErr(i, v, "int64")
	}
}

// Uint64 reads an unsigned integer column (any unsigned width, or year) as uint64.
func (r Row) Uint64(i int) (uint64, error) {
	v, err := r.at(i)
	if err != nil {
		return 0, err
	}
	switch v.Tag() {
	case my.DbValueTinyintUnsigned:
		return uint64(v.TinyintUnsigned()), nil
	case my.DbValueSmallintUnsigned:
		return uint64(v.SmallintUnsigned()), nil
	case my.DbValueMediumintUnsigned:
		return uint64(v.MediumintUnsigned()), nil
	case my.DbValueIntUnsigned:
		return uint64(v.IntUnsigned()), nil
	case my.DbValueBigintUnsigned:
		return v.BigintUnsigned(), nil
	case my.DbValueYear:
		return uint64(v.Year()), nil
	default:
		return 0, typeErr(i, v, "uint64")
	}
}

// Float64 reads a float or double column as float64.
func (r Row) Float64(i int) (float64, error) {
	v, err := r.at(i)
	if err != nil {
		return 0, err
	}
	switch v.Tag() {
	case my.DbValueDouble:
		return v.Double(), nil
	case my.DbValueFloat:
		return float64(v.Float()), nil
	default:
		return 0, typeErr(i, v, "float64")
	}
}

// String reads a textual column (the char/text families, decimal, json, enum or
// set) as a string.
func (r Row) String(i int) (string, error) {
	v, err := r.at(i)
	if err != nil {
		return "", err
	}
	switch v.Tag() {
	case my.DbValueFixchar:
		return v.Fixchar(), nil
	case my.DbValueVarchar:
		return v.Varchar(), nil
	case my.DbValueTinytext:
		return v.Tinytext(), nil
	case my.DbValueText:
		return v.Text(), nil
	case my.DbValueMediumtext:
		return v.Mediumtext(), nil
	case my.DbValueLongtext:
		return v.Longtext(), nil
	case my.DbValueDecimal:
		return v.Decimal(), nil
	case my.DbValueJson:
		return v.Json(), nil
	case my.DbValueEnumeration:
		return v.Enumeration(), nil
	case my.DbValueSet:
		return v.Set(), nil
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
	if v.Tag() != my.DbValueBoolean {
		return false, typeErr(i, v, "bool")
	}
	return v.Boolean(), nil
}

// Bytes reads a binary or blob column.
func (r Row) Bytes(i int) ([]byte, error) {
	v, err := r.at(i)
	if err != nil {
		return nil, err
	}
	switch v.Tag() {
	case my.DbValueBinary:
		return v.Binary(), nil
	case my.DbValueVarbinary:
		return v.Varbinary(), nil
	case my.DbValueTinyblob:
		return v.Tinyblob(), nil
	case my.DbValueBlob:
		return v.Blob(), nil
	case my.DbValueMediumblob:
		return v.Mediumblob(), nil
	case my.DbValueLongblob:
		return v.Longblob(), nil
	default:
		return nil, typeErr(i, v, "bytes")
	}
}

// Time reads a date, datetime or timestamp column as a [time.Time] (in UTC).
func (r Row) Time(i int) (time.Time, error) {
	v, err := r.at(i)
	if err != nil {
		return time.Time{}, err
	}
	switch v.Tag() {
	case my.DbValueDate:
		return dateToGoTime(v.Date()), nil
	case my.DbValueDatetime:
		return timestampToGoTime(v.Datetime()), nil
	case my.DbValueTimestamp:
		return timestampToGoTime(v.Timestamp()), nil
	default:
		return time.Time{}, typeErr(i, v, "time.Time")
	}
}

// Scan decodes the row into the given pointers, one per column. Supported
// destinations are *int64, *int, *uint64, *float64, *string, *bool, *[]byte,
// *time.Time and *any (which receives whatever [Row.Get] returns).
func (r Row) Scan(dest ...any) error {
	if len(dest) != len(r.values) {
		return fmt.Errorf("golem/rdbms/mysql: Scan: %d destination(s) for %d column(s)", len(dest), len(r.values))
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
	case *uint64:
		v, err := r.Uint64(i)
		if err != nil {
			return err
		}
		*p = v
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
	case *time.Time:
		v, err := r.Time(i)
		if err != nil {
			return err
		}
		*p = v
	default:
		return fmt.Errorf("golem/rdbms/mysql: Scan: unsupported destination type %T for column %d", d, i)
	}
	return nil
}

// ResultSet is the eager result of a [DB.Query] / [Tx.Query]: all columns and
// rows, already fetched.
type ResultSet struct {
	columns []Column
	rows    []Row
}

func newResultSet(res my.DbResult) *ResultSet {
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
	raw  *my.DbResultStream
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

// DB is an open MySQL connection.
type DB struct{ raw *my.DbConnection }

// Open opens a connection to the given MySQL URL
// ("mysql://user:pass@host:port/database").
func Open(address string) (*DB, error) {
	r := my.DbConnectionOpen(address)
	if r.IsErr() {
		return nil, myError(r.Err())
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
		return nil, myError(r.Err())
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
		return 0, myError(r.Err())
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
		return nil, myError(r.Err())
	}
	return &Stream{raw: r.Ok()}, nil
}

// Begin starts a transaction.
func (db *DB) Begin() (*Tx, error) {
	r := db.raw.BeginTransaction()
	if r.IsErr() {
		return nil, myError(r.Err())
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
type Tx struct{ raw *my.DbTransaction }

// Query runs a row-returning statement in the transaction.
func (tx *Tx) Query(sql string, args ...any) (*ResultSet, error) {
	params, err := encodeParams(args)
	if err != nil {
		return nil, err
	}
	r := tx.raw.Query(sql, params)
	if r.IsErr() {
		return nil, myError(r.Err())
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
		return 0, myError(r.Err())
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
		return nil, myError(r.Err())
	}
	return &Stream{raw: r.Ok()}, nil
}

// Commit commits the transaction.
func (tx *Tx) Commit() error {
	r := tx.raw.Commit()
	tx.raw.Drop()
	if r.IsErr() {
		return myError(r.Err())
	}
	return nil
}

// Rollback aborts the transaction.
func (tx *Tx) Rollback() error {
	r := tx.raw.Rollback()
	tx.raw.Drop()
	if r.IsErr() {
		return myError(r.Err())
	}
	return nil
}
