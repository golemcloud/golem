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

// Package rdbms holds the temporal and network value types shared by the Golem
// postgres and mysql drivers. The everyday cases — timestamp,
// timestamptz and date — round-trip through the standard library's time.Time
// (see the driver's Row.Time getter); these structs exist for the cases time.Time
// cannot hold on its own (a bare time of day, a time with an offset, an interval)
// and for building typed parameters via the driver's constructors.
package rdbms

import (
	"fmt"
	"math"
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

// Validate reports a field outside the range a database date holds.
func (d Date) Validate() error {
	switch {
	case d.Year < math.MinInt32 || d.Year > math.MaxInt32:
		return fmt.Errorf("date %s: year %d is out of range", d, d.Year)
	case d.Month < 1 || d.Month > 12:
		return fmt.Errorf("date %s: month %d is not 1..12", d, d.Month)
	case d.Day < 1 || d.Day > 31:
		return fmt.Errorf("date %s: day %d is not 1..31", d, d.Day)
	}
	return nil
}

// Validate reports a field outside the range a database time of day holds. A
// second of 60 is a leap second.
func (t Time) Validate() error {
	switch {
	case t.Hour < 0 || t.Hour > 23:
		return fmt.Errorf("time %s: hour %d is not 0..23", t, t.Hour)
	case t.Minute < 0 || t.Minute > 59:
		return fmt.Errorf("time %s: minute %d is not 0..59", t, t.Minute)
	case t.Second < 0 || t.Second > 60:
		return fmt.Errorf("time %s: second %d is not 0..60", t, t.Second)
	case t.Nanosecond < 0 || t.Nanosecond > 999_999_999:
		return fmt.Errorf("time %s: nanosecond %d is not 0..999999999", t, t.Nanosecond)
	}
	return nil
}

// Validate reports a field outside the range a database time with offset holds.
func (t Timetz) Validate() error {
	if err := t.Time.Validate(); err != nil {
		return err
	}
	return validOffset(t.OffsetSeconds)
}

// Validate reports a field outside the range a database timestamp holds.
func (ts Timestamp) Validate() error {
	if err := ts.Date.Validate(); err != nil {
		return err
	}
	return ts.Time.Validate()
}

// Validate reports a field outside the range a database timestamp with offset
// holds.
func (ts Timestamptz) Validate() error {
	if err := ts.Timestamp.Validate(); err != nil {
		return err
	}
	return validOffset(ts.OffsetSeconds)
}

// Validate reports a field outside the range a database interval holds.
func (iv Interval) Validate() error {
	switch {
	case iv.Months < math.MinInt32 || iv.Months > math.MaxInt32:
		return fmt.Errorf("interval: months %d is out of range", iv.Months)
	case iv.Days < math.MinInt32 || iv.Days > math.MaxInt32:
		return fmt.Errorf("interval: days %d is out of range", iv.Days)
	}
	return nil
}

// validOffset accepts a UTC offset of less than a day either way.
func validOffset(seconds int) error {
	if seconds <= -86400 || seconds >= 86400 {
		return fmt.Errorf("UTC offset of %d seconds is not within a day", seconds)
	}
	return nil
}

// MacAddr is a six-octet hardware address.
type MacAddr [6]byte

func (m MacAddr) String() string {
	return fmt.Sprintf("%02x:%02x:%02x:%02x:%02x:%02x", m[0], m[1], m[2], m[3], m[4], m[5])
}

// ErrorKind classifies a database error. It is shared by the postgres and mysql
// drivers.
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
