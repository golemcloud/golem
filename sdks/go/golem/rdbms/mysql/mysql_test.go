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

package mysql

import (
	"testing"
	"time"

	my "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_rdbms_mysql"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/types"
)

// TestEncodeParamTags — each Go value coerces to the expected db-value family,
// including the signed/unsigned integer split MySQL supports natively.
func TestEncodeParamTags(t *testing.T) {
	cases := []struct {
		in   any
		want uint8
	}{
		{nil, my.DbValueNull},
		{true, my.DbValueBoolean},
		{int(1), my.DbValueBigint},
		{int64(1), my.DbValueBigint},
		{int32(1), my.DbValueInt},
		{int16(1), my.DbValueSmallint},
		{int8(1), my.DbValueTinyint},
		{uint(1), my.DbValueBigintUnsigned},
		{uint64(1), my.DbValueBigintUnsigned},
		{uint32(1), my.DbValueIntUnsigned},
		{uint16(1), my.DbValueSmallintUnsigned},
		{uint8(1), my.DbValueTinyintUnsigned},
		{float64(1), my.DbValueDouble},
		{float32(1), my.DbValueFloat},
		{"s", my.DbValueVarchar},
		{[]byte{1}, my.DbValueBlob},
		{time.Now().UTC(), my.DbValueDatetime},
		{Decimal("1.5"), my.DbValueDecimal},
		{JSON(`{}`), my.DbValueJson},
		{Year(2026), my.DbValueYear},
	}
	for _, c := range cases {
		v, err := encodeParam(c.in)
		if err != nil {
			t.Fatalf("encodeParam(%T) error: %v", c.in, err)
		}
		if v.Tag() != c.want {
			t.Fatalf("encodeParam(%T) tag = %s, want %s", c.in, tagName(v.Tag()), tagName(c.want))
		}
	}
}

// TestEncodeParamUnsupported — an unhandled type is a clear error, not a panic.
func TestEncodeParamUnsupported(t *testing.T) {
	if _, err := encodeParam(struct{ X int }{}); err == nil {
		t.Fatal("expected error for unsupported type")
	}
}

// TestTypedGetters — typed getters read the matching families and reject others.
func TestTypedGetters(t *testing.T) {
	r := Row{values: []my.DbValue{
		my.MakeDbValueBigint(42),
		my.MakeDbValueDouble(3.5),
		my.MakeDbValueVarchar("hello"),
		my.MakeDbValueBoolean(true),
		my.MakeDbValueBlob([]byte{1, 2, 3}),
		my.MakeDbValueNull(),
	}}

	if got, err := r.Int64(0); err != nil || got != 42 {
		t.Fatalf("Int64 = %d, %v", got, err)
	}
	if got, err := r.Float64(1); err != nil || got != 3.5 {
		t.Fatalf("Float64 = %v, %v", got, err)
	}
	if got, err := r.String(2); err != nil || got != "hello" {
		t.Fatalf("String = %q, %v", got, err)
	}
	if got, err := r.Bool(3); err != nil || !got {
		t.Fatalf("Bool = %v, %v", got, err)
	}
	if got, err := r.Bytes(4); err != nil || len(got) != 3 || got[2] != 3 {
		t.Fatalf("Bytes = %v, %v", got, err)
	}
	if !r.IsNull(5) {
		t.Fatal("column 5 should be null")
	}
	if _, err := r.Int64(2); err == nil {
		t.Fatal("Int64 on varchar should error")
	}
	if _, err := r.Int64(99); err == nil {
		t.Fatal("Int64 out of range should error")
	}
}

// TestUnsignedGetters — unsigned families read through Int64 and Uint64.
func TestUnsignedGetters(t *testing.T) {
	big := uint64(1) << 63 // above math.MaxInt64
	r := Row{values: []my.DbValue{
		my.MakeDbValueIntUnsigned(7),
		my.MakeDbValueBigintUnsigned(big),
		my.MakeDbValueYear(2026),
	}}
	if got, err := r.Uint64(1); err != nil || got != big {
		t.Fatalf("Uint64(bigint-unsigned) = %d, %v", got, err)
	}
	if got, err := r.Int64(0); err != nil || got != 7 {
		t.Fatalf("Int64(int-unsigned) = %d, %v", got, err)
	}
	if got, err := r.Uint64(2); err != nil || got != 2026 {
		t.Fatalf("Uint64(year) = %d, %v", got, err)
	}
}

// TestScan — Scan fills typed destinations positionally, including *uint64 and *any.
func TestScan(t *testing.T) {
	r := Row{values: []my.DbValue{
		my.MakeDbValueBigint(7),
		my.MakeDbValueVarchar("name"),
		my.MakeDbValueBigintUnsigned(9),
	}}
	var id int64
	var name string
	var u uint64
	if err := r.Scan(&id, &name, &u); err != nil {
		t.Fatalf("Scan error: %v", err)
	}
	if id != 7 || name != "name" || u != 9 {
		t.Fatalf("Scan = %d %q %d", id, name, u)
	}

	var anyVal any
	if err := r.Scan(&anyVal, new(string), new(uint64)); err != nil {
		t.Fatalf("Scan into *any error: %v", err)
	}
	if anyVal.(int64) != 7 {
		t.Fatalf("*any = %v", anyVal)
	}

	if err := r.Scan(&id); err == nil {
		t.Fatal("Scan with wrong count should error")
	}
}

// TestDecodeValue — decode maps each family to the expected Go type.
func TestDecodeValue(t *testing.T) {
	if v := decodeValue(my.MakeDbValueNull()); v != nil {
		t.Fatalf("null = %v", v)
	}
	if v := decodeValue(my.MakeDbValueInt(5)); v.(int32) != 5 {
		t.Fatalf("int = %v", v)
	}
	if v := decodeValue(my.MakeDbValueBigintUnsigned(9)); v.(uint64) != 9 {
		t.Fatalf("bigint-unsigned = %v", v)
	}
	if v := decodeValue(my.MakeDbValueDecimal("1.25")); v.(string) != "1.25" {
		t.Fatalf("decimal = %v", v)
	}
	if v := decodeValue(my.MakeDbValueYear(2026)); v.(uint16) != 2026 {
		t.Fatalf("year = %v", v)
	}
	if v := decodeValue(my.MakeDbValueSet("a,b")); v.(string) != "a,b" {
		t.Fatalf("set = %v", v)
	}
}

// TestTimeRoundTrip — time.Time survives the datetime encode/decode (UTC).
func TestTimeRoundTrip(t *testing.T) {
	orig := time.Date(2026, 7, 30, 12, 34, 56, 0, time.UTC)
	got := timestampToGoTime(goTimeToTimestamp(orig))
	if !got.Equal(orig) {
		t.Fatalf("datetime round-trip = %v, want %v", got, orig)
	}
	tm := decodeValue(my.MakeDbValueTime(my.Time{Hour: 1, Minute: 2, Second: 3, Nanosecond: 4})).(types.Time)
	if tm.Hour != 1 || tm.Minute != 2 || tm.Second != 3 || tm.Nanosecond != 4 {
		t.Fatalf("time = %+v", tm)
	}
}

// TestTagName — display names key on the generated constants, with a diagnosable fallback.
func TestTagName(t *testing.T) {
	cases := []struct {
		tag  uint8
		want string
	}{
		{my.DbValueBoolean, "boolean"},
		{my.DbValueBigintUnsigned, "bigint-unsigned"},
		{my.DbValueDecimal, "decimal"},
		{my.DbValueJson, "json"},
		{200, "unknown(200)"},
	}
	for _, c := range cases {
		if got := tagName(c.tag); got != c.want {
			t.Fatalf("tagName(%d) = %q, want %q", c.tag, got, c.want)
		}
	}
}

// TestErrorMapping — each host error variant maps to the right kind/message.
func TestErrorMapping(t *testing.T) {
	cases := []struct {
		raw  my.Error
		kind ErrorKind
		msg  string
	}{
		{my.MakeErrorConnectionFailure("c"), ConnectionFailure, "c"},
		{my.MakeErrorQueryParameterFailure("p"), QueryParameterFailure, "p"},
		{my.MakeErrorQueryExecutionFailure("e"), QueryExecutionFailure, "e"},
		{my.MakeErrorQueryResponseFailure("r"), QueryResponseFailure, "r"},
		{my.MakeErrorOther("o"), Other, "o"},
	}
	for _, c := range cases {
		err := myError(c.raw).(*Error)
		if err.Kind != c.kind || err.Message != c.msg {
			t.Fatalf("mapping = kind %d msg %q, want kind %d msg %q", err.Kind, err.Message, c.kind, c.msg)
		}
	}
}
