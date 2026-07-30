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

package postgres

import (
	"net/netip"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	pg "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_rdbms_postgres"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/types"
)

// TestEncodeParamTags — each Go value coerces to the expected db-value family.
func TestEncodeParamTags(t *testing.T) {
	cases := []struct {
		in   any
		want uint8
	}{
		{nil, pg.DbValueNull},
		{true, pg.DbValueBoolean},
		{int(1), pg.DbValueInt8},
		{int64(1), pg.DbValueInt8},
		{int32(1), pg.DbValueInt4},
		{int16(1), pg.DbValueInt2},
		{int8(1), pg.DbValueInt2},
		{float64(1), pg.DbValueFloat8},
		{float32(1), pg.DbValueFloat4},
		{"s", pg.DbValueText},
		{[]byte{1}, pg.DbValueBytea},
		{golem.UUID{}, pg.DbValueUuid},
		{time.Now().UTC(), pg.DbValueTimestamptz},
		{Numeric("1.5"), pg.DbValueNumeric},
		{JSONB(`{}`), pg.DbValueJsonb},
		{Int4(3), pg.DbValueInt4},
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
	if _, err := encodeParam(uint64(1)); err == nil {
		t.Fatal("expected error for uint64 (use a constructor)")
	}
}

// TestTypedGetters — typed getters read the matching families and reject others.
func TestTypedGetters(t *testing.T) {
	r := Row{values: []pg.DbValue{
		pg.MakeDbValueInt8(42),
		pg.MakeDbValueFloat8(3.5),
		pg.MakeDbValueText("hello"),
		pg.MakeDbValueBoolean(true),
		pg.MakeDbValueBytea([]byte{1, 2, 3}),
		pg.MakeDbValueNull(),
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
	if r.IsNull(0) {
		t.Fatal("column 0 should not be null")
	}

	// Mismatch is an error, not a panic.
	if _, err := r.Int64(2); err == nil {
		t.Fatal("Int64 on text should error")
	}
	// Out of range.
	if _, err := r.Int64(99); err == nil {
		t.Fatal("Int64 out of range should error")
	}
}

// TestInt64Widths — every integer family reads through Int64.
func TestInt64Widths(t *testing.T) {
	r := Row{values: []pg.DbValue{
		pg.MakeDbValueInt2(2),
		pg.MakeDbValueInt4(4),
		pg.MakeDbValueInt8(8),
		pg.MakeDbValueCharacter(1),
		pg.MakeDbValueMoney(100),
		pg.MakeDbValueOid(7),
	}}
	want := []int64{2, 4, 8, 1, 100, 7}
	for i, w := range want {
		if got, err := r.Int64(i); err != nil || got != w {
			t.Fatalf("Int64(%d) = %d, %v; want %d", i, got, err, w)
		}
	}
}

// TestScan — Scan fills typed destinations positionally, including *any.
func TestScan(t *testing.T) {
	u := golem.UUID{0: 0xab, 15: 0x01}
	r := Row{values: []pg.DbValue{
		pg.MakeDbValueInt8(7),
		pg.MakeDbValueText("name"),
		pg.MakeDbValueBoolean(false),
		pg.MakeDbValueUuid(uuidToWit(u)),
	}}
	var id int64
	var name string
	var ok bool
	var got golem.UUID
	if err := r.Scan(&id, &name, &ok, &got); err != nil {
		t.Fatalf("Scan error: %v", err)
	}
	if id != 7 || name != "name" || ok || got != u {
		t.Fatalf("Scan = %d %q %v %v", id, name, ok, got)
	}

	// *any receives the decoded value.
	var anyVal any
	if err := r.Scan(&anyVal, new(string), new(bool), new(golem.UUID)); err != nil {
		t.Fatalf("Scan into *any error: %v", err)
	}
	if anyVal.(int64) != 7 {
		t.Fatalf("*any = %v", anyVal)
	}

	// Wrong destination count.
	if err := r.Scan(&id); err == nil {
		t.Fatal("Scan with wrong count should error")
	}
}

// TestDecodeValue — decode maps each family to the expected Go type.
func TestDecodeValue(t *testing.T) {
	if v := decodeValue(pg.MakeDbValueNull()); v != nil {
		t.Fatalf("null = %v", v)
	}
	if v := decodeValue(pg.MakeDbValueInt4(5)); v.(int32) != 5 {
		t.Fatalf("int4 = %v", v)
	}
	if v := decodeValue(pg.MakeDbValueNumeric("1.25")); v.(string) != "1.25" {
		t.Fatalf("numeric = %v", v)
	}
	if v := decodeValue(pg.MakeDbValueJsonb(`{"a":1}`)); v.(string) != `{"a":1}` {
		t.Fatalf("jsonb = %v", v)
	}
	if v := decodeValue(pg.MakeDbValueVector([]float32{1, 2})); len(v.([]float32)) != 2 {
		t.Fatalf("vector = %v", v)
	}

	// A family we do not decode yet comes back as an opaque DbValue.
	esc := decodeValue(pg.MakeDbValueEnumeration(pg.Enumeration{Name: "color", Value: "red"}))
	dv, ok := esc.(DbValue)
	if !ok {
		t.Fatalf("enumeration should decode to DbValue, got %T", esc)
	}
	if dv.Kind() != "enumeration" {
		t.Fatalf("Kind = %q", dv.Kind())
	}
}

// TestTagName — display names key on the generated constants, with a diagnosable
// fallback for an unrecognized (e.g. newly appended) tag.
func TestTagName(t *testing.T) {
	cases := []struct {
		tag  uint8
		want string
	}{
		{pg.DbValueCharacter, "character"},
		{pg.DbValueNumeric, "numeric"},
		{pg.DbValueUuid, "uuid"},
		{pg.DbValueSparsevec, "sparsevec"},
		{200, "unknown(200)"},
	}
	for _, c := range cases {
		if got := tagName(c.tag); got != c.want {
			t.Fatalf("tagName(%d) = %q, want %q", c.tag, got, c.want)
		}
	}
}

// TestTimeRoundTrip — time.Time survives the timestamptz encode/decode.
func TestTimeRoundTrip(t *testing.T) {
	orig := time.Date(2026, 7, 30, 12, 34, 56, 123456000, time.FixedZone("", 2*3600))
	got := timestamptzToGoTime(goTimeToTimestamptz(orig))
	if !got.Equal(orig) {
		t.Fatalf("timestamptz round-trip = %v, want %v", got, orig)
	}
	if _, off := got.Zone(); off != 2*3600 {
		t.Fatalf("offset = %d, want %d", off, 2*3600)
	}

	// A date decodes to midnight UTC.
	d := dateToGoTime(pg.Date{Year: 2026, Month: 7, Day: 30})
	if d.Year() != 2026 || d.Month() != 7 || d.Day() != 30 || d.Hour() != 0 {
		t.Fatalf("date = %v", d)
	}
}

// TestTemporalTypes — time/timetz/interval decode into the types structs.
func TestTemporalTypes(t *testing.T) {
	tm := decodeValue(pg.MakeDbValueTime(pg.Time{Hour: 1, Minute: 2, Second: 3, Nanosecond: 4})).(types.Time)
	if tm.Hour != 1 || tm.Minute != 2 || tm.Second != 3 || tm.Nanosecond != 4 {
		t.Fatalf("time = %+v", tm)
	}
	iv := decodeValue(pg.MakeDbValueInterval(pg.Interval{Months: 1, Days: 2, Microseconds: 3})).(types.Interval)
	if iv.Months != 1 || iv.Days != 2 || iv.Microseconds != 3 {
		t.Fatalf("interval = %+v", iv)
	}
}

// TestUUIDRoundTrip — the big/low-bits split is order-preserving.
func TestUUIDRoundTrip(t *testing.T) {
	var u golem.UUID
	for i := range u {
		u[i] = byte(i + 1)
	}
	if got := uuidFromWit(uuidToWit(u)); got != u {
		t.Fatalf("uuid round-trip = %v, want %v", got, u)
	}
}

// TestNetRoundTrip — inet (v4 and v6) and macaddr survive conversion.
func TestNetRoundTrip(t *testing.T) {
	for _, s := range []string{"192.168.1.5", "2001:db8::1"} {
		a := netip.MustParseAddr(s)
		got := ipFromWit(ipToWit(a))
		if got != a {
			t.Fatalf("ip round-trip %q = %v", s, got)
		}
	}
	m := types.MacAddr{0xde, 0xad, 0xbe, 0xef, 0x00, 0x01}
	if got := macFromWit(macToWit(m)); got != m {
		t.Fatalf("mac round-trip = %v", got)
	}
}

// TestErrorMapping — each host error variant maps to the right kind/message.
func TestErrorMapping(t *testing.T) {
	cases := []struct {
		raw  pg.Error
		kind ErrorKind
		msg  string
	}{
		{pg.MakeErrorConnectionFailure("c"), ConnectionFailure, "c"},
		{pg.MakeErrorQueryParameterFailure("p"), QueryParameterFailure, "p"},
		{pg.MakeErrorQueryExecutionFailure("e"), QueryExecutionFailure, "e"},
		{pg.MakeErrorQueryResponseFailure("r"), QueryResponseFailure, "r"},
		{pg.MakeErrorOther("o"), Other, "o"},
	}
	for _, c := range cases {
		err := pgError(c.raw).(*Error)
		if err.Kind != c.kind || err.Message != c.msg {
			t.Fatalf("mapping = kind %d msg %q, want kind %d msg %q", err.Kind, err.Message, c.kind, c.msg)
		}
	}
}
