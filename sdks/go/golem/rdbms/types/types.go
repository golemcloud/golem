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

// Package types holds the temporal and network value types shared by the Golem
// rdbms drivers (postgres today, mysql later). The everyday cases — timestamp,
// timestamptz and date — round-trip through the standard library's time.Time
// (see the driver's Row.Time getter); these structs exist for the cases time.Time
// cannot hold on its own (a bare time of day, a time with an offset, an interval)
// and for building typed parameters via the driver's constructors.
package types

import (
	"fmt"
	"time"
)

// Date is a calendar date with no time zone.
type Date struct{ Year, Month, Day int }

// DateOf takes the date part of t (in t's own location).
func DateOf(t time.Time) Date {
	y, m, d := t.Date()
	return Date{Year: y, Month: int(m), Day: d}
}

// ToTime renders the date as midnight UTC.
func (d Date) ToTime() time.Time {
	return time.Date(d.Year, time.Month(d.Month), d.Day, 0, 0, 0, 0, time.UTC)
}

func (d Date) String() string { return fmt.Sprintf("%04d-%02d-%02d", d.Year, d.Month, d.Day) }

// Time is a time of day with no time zone.
type Time struct{ Hour, Minute, Second, Nanosecond int }

// TimeOf takes the clock part of t (in t's own location).
func TimeOf(t time.Time) Time {
	return Time{Hour: t.Hour(), Minute: t.Minute(), Second: t.Second(), Nanosecond: t.Nanosecond()}
}

func (t Time) String() string {
	return fmt.Sprintf("%02d:%02d:%02d.%09d", t.Hour, t.Minute, t.Second, t.Nanosecond)
}

// Timetz is a time of day with a UTC offset (seconds east of UTC).
type Timetz struct {
	Time          Time
	OffsetSeconds int
}

// Timestamp is a date and time with no time zone.
type Timestamp struct {
	Date Date
	Time Time
}

// TimestampOf splits t (in t's own location) into a Timestamp.
func TimestampOf(t time.Time) Timestamp {
	return Timestamp{Date: DateOf(t), Time: TimeOf(t)}
}

// ToTime renders the timestamp in UTC.
func (ts Timestamp) ToTime() time.Time {
	return time.Date(ts.Date.Year, time.Month(ts.Date.Month), ts.Date.Day,
		ts.Time.Hour, ts.Time.Minute, ts.Time.Second, ts.Time.Nanosecond, time.UTC)
}

// Timestamptz is a Timestamp with a UTC offset (seconds east of UTC).
type Timestamptz struct {
	Timestamp     Timestamp
	OffsetSeconds int
}

// TimestamptzOf splits t into a Timestamptz, preserving t's offset.
func TimestamptzOf(t time.Time) Timestamptz {
	_, off := t.Zone()
	return Timestamptz{Timestamp: TimestampOf(t), OffsetSeconds: off}
}

// ToTime reconstructs the instant in a fixed zone matching the stored offset.
func (ts Timestamptz) ToTime() time.Time {
	loc := time.FixedZone("", ts.OffsetSeconds)
	t := ts.Timestamp.ToTime()
	return time.Date(t.Year(), t.Month(), t.Day(), t.Hour(), t.Minute(), t.Second(), t.Nanosecond(), loc)
}

// Interval is a Postgres interval: a whole number of months and days plus a
// sub-day microsecond span.
type Interval struct {
	Months       int
	Days         int
	Microseconds int64
}

// MacAddr is a six-octet hardware address.
type MacAddr [6]byte

func (m MacAddr) String() string {
	return fmt.Sprintf("%02x:%02x:%02x:%02x:%02x:%02x", m[0], m[1], m[2], m[3], m[4], m[5])
}
