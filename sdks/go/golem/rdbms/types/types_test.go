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

package types

import (
	"testing"
	"time"
)

// TestTimestampOfToTime — time.Time round-trips through Timestamp (UTC).
func TestTimestampOfToTime(t *testing.T) {
	orig := time.Date(2026, 7, 30, 12, 34, 56, 789, time.UTC)
	if got := TimestampOf(orig).ToTime(); !got.Equal(orig) {
		t.Fatalf("Timestamp round-trip = %v, want %v", got, orig)
	}
}

// TestTimestamptzOfToTime — Timestamptz preserves the offset.
func TestTimestamptzOfToTime(t *testing.T) {
	loc := time.FixedZone("", -5*3600)
	orig := time.Date(2026, 1, 2, 3, 4, 5, 0, loc)
	tz := TimestamptzOf(orig)
	if tz.OffsetSeconds != -5*3600 {
		t.Fatalf("offset = %d", tz.OffsetSeconds)
	}
	if got := tz.ToTime(); !got.Equal(orig) {
		t.Fatalf("Timestamptz round-trip = %v, want %v", got, orig)
	}
}

// TestDateStringAndTime — Date renders and converts as expected.
func TestDateStringAndTime(t *testing.T) {
	d := Date{Year: 2026, Month: 7, Day: 5}
	if d.String() != "2026-07-05" {
		t.Fatalf("Date.String = %q", d.String())
	}
	if tm := d.ToTime(); tm.Hour() != 0 || tm.Day() != 5 {
		t.Fatalf("Date.ToTime = %v", tm)
	}
	if got := DateOf(time.Date(2026, 7, 5, 9, 0, 0, 0, time.UTC)); got != d {
		t.Fatalf("DateOf = %+v", got)
	}
}

// TestMacAddrString — canonical colon-separated hex.
func TestMacAddrString(t *testing.T) {
	m := MacAddr{0xde, 0xad, 0xbe, 0xef, 0x00, 0x01}
	if m.String() != "de:ad:be:ef:00:01" {
		t.Fatalf("MacAddr.String = %q", m.String())
	}
}
