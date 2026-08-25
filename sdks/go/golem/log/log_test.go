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

package log

import (
	"context"
	"log/slog"
	"testing"
	"time"
)

// TestMapLevel — slog levels fold onto the six wasi:logging severities, including
// the trace/critical ends slog has no named constants for.
func TestMapLevel(t *testing.T) {
	cases := []struct {
		in   slog.Level
		want Level
	}{
		{slog.LevelDebug - 1, Trace},
		{slog.LevelDebug, Debug},
		{slog.LevelInfo, Info},
		{slog.LevelInfo + 1, Info},
		{slog.LevelWarn, Warn},
		{slog.LevelError, Error},
		{slog.LevelError + 3, Error},
		{slog.LevelError + 4, Critical},
	}
	for _, c := range cases {
		if got := mapLevel(c.in); got != c.want {
			t.Fatalf("mapLevel(%v) = %d, want %d", c.in, got, c.want)
		}
	}
}

// TestFormatMessage — message plus handler and record attrs render as "k=v".
func TestFormatMessage(t *testing.T) {
	r := slog.NewRecord(time.Time{}, slog.LevelInfo, "charged", 0)
	r.AddAttrs(slog.Int("amount", 500), slog.String("cur", "usd"))
	base := []slog.Attr{slog.String("area", "billing")}
	got := formatMessage(base, r)
	want := "charged area=billing amount=500 cur=usd"
	if got != want {
		t.Fatalf("formatMessage = %q, want %q", got, want)
	}

	// No attrs → just the message.
	r2 := slog.NewRecord(time.Time{}, slog.LevelWarn, "plain", 0)
	if got := formatMessage(nil, r2); got != "plain" {
		t.Fatalf("formatMessage(plain) = %q", got)
	}
}

// TestEnabled — the minimum level gates records; nil options default to Info.
func TestEnabled(t *testing.T) {
	ctx := context.Background()
	h := NewHandler(nil)
	if h.Enabled(ctx, slog.LevelDebug) {
		t.Fatal("debug should be disabled at default Info")
	}
	if !h.Enabled(ctx, slog.LevelInfo) || !h.Enabled(ctx, slog.LevelError) {
		t.Fatal("info/error should be enabled at default Info")
	}

	dbg := NewHandler(&Options{Level: slog.LevelDebug})
	if !dbg.Enabled(ctx, slog.LevelDebug) {
		t.Fatal("debug should be enabled when min level is Debug")
	}
}

// TestWithGroupAndAttrs — WithGroup builds the context path; WithAttrs accumulates.
func TestWithGroupAndAttrs(t *testing.T) {
	base := NewHandler(&Options{Context: "app"})

	g := base.WithGroup("billing").(*Handler)
	if g.ctx != "app/billing" {
		t.Fatalf("ctx = %q, want app/billing", g.ctx)
	}
	g2 := g.WithGroup("retry").(*Handler)
	if g2.ctx != "app/billing/retry" {
		t.Fatalf("ctx = %q", g2.ctx)
	}

	a := g.WithAttrs([]slog.Attr{slog.Int("id", 7)}).(*Handler)
	if len(a.attrs) != 1 || a.attrs[0].Key != "id" {
		t.Fatalf("attrs = %+v", a.attrs)
	}
	// WithAttrs must not mutate the receiver.
	if len(g.attrs) != 0 {
		t.Fatalf("receiver mutated: %+v", g.attrs)
	}

	// Empty name/attrs are no-ops that return the same handler.
	if base.WithGroup("") != slog.Handler(base) {
		t.Fatal("WithGroup(\"\") should return the receiver")
	}
}
