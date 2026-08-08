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

// Package log routes Go logging through Golem's structured host logging channel
// (wasi:logging). Unlike writing to stdout/stderr — which the host records as raw
// bytes with no level — a wasi:logging record carries a typed level and a context
// (category) string, so it shows up in worker logs and the oplog with the right
// severity.
//
// The Golem runtime installs this as the default slog handler on agent startup,
// so ordinary structured logging just works:
//
//	slog.Info("charged", "amount", 500)          // -> level=info
//	slog.Warn("retrying", "attempt", 2)          // -> level=warn
//	slog.With("area", "billing").Error("declined") // context carries the group
//
// (Because Go's slog.SetDefault also bridges the standard log package, plain
// log.Print output flows here too.) Call [SetDefault] to change the minimum level
// or base context, [NewHandler] to build a handler yourself, or [Log] for a raw
// one-shot record.
package log

import (
	"context"
	"io"
	"log/slog"
	"strings"

	witlog "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_logging_logging"
)

// Level is a wasi:logging severity.
type Level = witlog.Level

// The severity levels, in increasing order.
const (
	Trace    Level = witlog.LevelTrace
	Debug    Level = witlog.LevelDebug
	Info     Level = witlog.LevelInfo
	Warn     Level = witlog.LevelWarn
	Error    Level = witlog.LevelError
	Critical Level = witlog.LevelCritical
)

// Log emits a single record to the host logging channel. context is a free-form
// category used to group related messages (it may be empty).
func Log(level Level, context, message string) { witlog.Log(level, context, message) }

// mapLevel maps an slog level onto the wasi:logging severities. slog has no
// trace/critical by default, so anything below Debug is trace and anything a full
// step above Error is critical.
func mapLevel(l slog.Level) Level {
	switch {
	case l < slog.LevelDebug:
		return Trace
	case l < slog.LevelInfo:
		return Debug
	case l < slog.LevelWarn:
		return Info
	case l < slog.LevelError:
		return Warn
	case l < slog.LevelError+4:
		return Error
	default:
		return Critical
	}
}

// formatMessage renders a record as "message key=value …", appending the
// handler's accumulated attributes and then the record's own. It is pure (no host
// call) so it is unit-tested directly.
func formatMessage(base []slog.Attr, r slog.Record) string {
	var b strings.Builder
	b.WriteString(r.Message)
	appendAttr := func(a slog.Attr) bool {
		if a.Equal(slog.Attr{}) {
			return true
		}
		b.WriteByte(' ')
		b.WriteString(a.Key)
		b.WriteByte('=')
		b.WriteString(a.Value.Resolve().String())
		return true
	}
	for _, a := range base {
		appendAttr(a)
	}
	r.Attrs(appendAttr)
	return b.String()
}

// Options configures a [Handler].
type Options struct {
	// Level is the minimum level to emit (nil means Info).
	Level slog.Leveler
	// Context is the base category string for every record.
	Context string
}

// Handler is an [slog.Handler] that writes to the host logging channel. Build one
// with [NewHandler].
type Handler struct {
	leveler slog.Leveler
	ctx     string
	attrs   []slog.Attr
}

// NewHandler builds a handler from opts (nil is fine — Info level, empty context).
func NewHandler(opts *Options) *Handler {
	h := &Handler{leveler: slog.LevelInfo}
	if opts != nil {
		if opts.Level != nil {
			h.leveler = opts.Level
		}
		h.ctx = opts.Context
	}
	return h
}

// Enabled reports whether a record at l would be emitted.
func (h *Handler) Enabled(_ context.Context, l slog.Level) bool {
	return l >= h.leveler.Level()
}

// Handle emits the record to the host logging channel.
func (h *Handler) Handle(_ context.Context, r slog.Record) error {
	Log(mapLevel(r.Level), h.ctx, formatMessage(h.attrs, r))
	return nil
}

// WithAttrs returns a handler that prepends attrs to every record.
func (h *Handler) WithAttrs(attrs []slog.Attr) slog.Handler {
	if len(attrs) == 0 {
		return h
	}
	nh := *h
	nh.attrs = append(append([]slog.Attr(nil), h.attrs...), attrs...)
	return &nh
}

// WithGroup extends the context (category) with name. slog groups map onto the
// wasi:logging context, so a grouped logger reads like a named category
// ("billing/retry") rather than key-prefixing attributes.
func (h *Handler) WithGroup(name string) slog.Handler {
	if name == "" {
		return h
	}
	nh := *h
	if nh.ctx == "" {
		nh.ctx = name
	} else {
		nh.ctx = nh.ctx + "/" + name
	}
	return &nh
}

// SetDefault installs a host-logging handler as slog's default logger (and, via
// slog, the standard log package). The Golem runtime calls this on startup; call
// it yourself to override the minimum level or base context.
func SetDefault(opts *Options) {
	slog.SetDefault(slog.New(NewHandler(opts)))
}

// Writer returns an io.Writer whose every write becomes one log record at the
// given level and context — handy for routing sinks that expect an io.Writer
// (e.g. a dedicated log.Logger). A trailing newline is trimmed.
func Writer(level Level, context string) io.Writer {
	return levelWriter{level: level, ctx: context}
}

type levelWriter struct {
	level Level
	ctx   string
}

func (w levelWriter) Write(p []byte) (int, error) {
	Log(w.level, w.ctx, strings.TrimRight(string(p), "\n"))
	return len(p), nil
}
