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

// Package durablestreams reads and writes external Durable Streams from inside
// an agent. The host does the HTTP, framing and authentication; a Reader keeps
// only its checkpoint and one buffered batch, and a Writer its producer
// progress and the one append in flight, all reconstructed by replay.
//
//	r := durablestreams.ReadJSON[Event](url, durablestreams.ReadOptions{})
//	for {
//	    ev, ok, err := r.Next()
//	    if err != nil || !ok { break }
//	    …
//	}
//
//	w, err := durablestreams.NewWriter(url, "application/json", durablestreams.WriteOptions{})
//	receipt, err := durablestreams.AppendJSON(w, []Event{ev}, false)
package durablestreams

import (
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	wire "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_durable_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Checkpoint is a position in a stream, as opaque server-generated tokens.
type Checkpoint struct {
	Offset string
	Cursor golem.Option[string]
}

// Start is the checkpoint before the first item.
var Start = Checkpoint{Offset: "-1", Cursor: golem.None[string]()}

// Transport is how a reader waits for new items once it has caught up.
type Transport uint8

const (
	CatchUp Transport = iota
	LongPoll
	SSE
)

// ErrorKind classifies a failed read or append.
type ErrorKind uint8

const (
	KindInvalidRequest ErrorKind = iota
	KindPermissionDenied
	KindNotFound
	KindGone
	KindClosed
	KindSequenceConflict
	KindFenced
	KindProducerDiverged
	KindProtocolError
	KindPayloadTooLarge
	KindTimeout
	KindTransport
	KindRateLimited
	KindUnavailable
)

// Error is a failed read or append. An error never means the end of a stream.
type Error struct {
	Kind             ErrorKind
	Message          string
	RetryAfter       golem.Option[time.Duration]
	ProducerEpoch    golem.Option[uint64]
	ExpectedSequence golem.Option[uint64]
}

func (e *Error) Error() string { return fmt.Sprintf("durable stream: %s", e.Message) }

func newError(kind ErrorKind, format string, args ...any) *Error {
	return &Error{
		Kind: kind, Message: fmt.Sprintf(format, args...),
		RetryAfter: golem.None[time.Duration](), ProducerEpoch: golem.None[uint64](), ExpectedSequence: golem.None[uint64](),
	}
}

func errorFromWire(e wire.DurableStreamError) *Error {
	out := newError(ErrorKind(e.Kind), "%s", e.Message)
	if e.RetryAfterMs.IsSome() {
		out.RetryAfter = golem.Some(time.Duration(e.RetryAfterMs.Some()) * time.Millisecond)
	}
	if e.ProducerEpoch.IsSome() {
		out.ProducerEpoch = golem.Some(e.ProducerEpoch.Some())
	}
	if e.ExpectedSequence.IsSome() {
		out.ExpectedSequence = golem.Some(e.ExpectedSequence.Some())
	}
	return out
}

// RetryOptions bounds the retries of consecutive transient failures (timeout,
// transport, rate limiting, unavailability), with exponential backoff.
type RetryOptions struct {
	MaxRetries   uint32
	InitialDelay time.Duration
	MaxDelay     time.Duration
}

// DefaultRetry is three retries from 100ms, up to 10s apart.
var DefaultRetry = RetryOptions{MaxRetries: 3, InitialDelay: 100 * time.Millisecond, MaxDelay: 10 * time.Second}

func (r RetryOptions) delay(err *Error, failures uint32) (time.Duration, bool) {
	switch err.Kind {
	case KindTimeout, KindTransport, KindRateLimited, KindUnavailable:
	default:
		return 0, false
	}
	if failures >= r.MaxRetries {
		return 0, false
	}
	backoff := min(r.InitialDelay<<min(failures, 62), r.MaxDelay)
	if wait, ok := err.RetryAfter.Get(); ok && wait > backoff {
		backoff = wait
	}
	return backoff, true
}

// ReadOptions configures a Reader; the zero value reads from [Start] with long
// polling, a 30s attempt deadline and [DefaultRetry].
type ReadOptions struct {
	Checkpoint Checkpoint
	// Transport is used once the reader has caught up; reading always starts
	// with a catch-up request.
	Transport Transport
	Timeout   time.Duration
	// IdleDelay is the pause after an empty batch at the tail.
	IdleDelay time.Duration
	Retry     RetryOptions
	// Auth authenticates the requests; its plaintext never reaches the agent.
	Auth golem.Option[golem.Secret[string]]
}

func (o ReadOptions) withDefaults() ReadOptions {
	if o.Checkpoint.Offset == "" {
		o.Checkpoint = Start
	}
	if o.Transport == CatchUp {
		o.Transport = LongPoll
	}
	if o.Timeout == 0 {
		o.Timeout = 30 * time.Second
	}
	if o.IdleDelay == 0 {
		o.IdleDelay = 100 * time.Millisecond
	}
	if o.Retry == (RetryOptions{}) {
		o.Retry = DefaultRetry
	}
	return o
}

// readCall performs one read attempt; the wasm build binds it to the host
// reader (durablestreams_wasm.go).
type readCall func(wire.DurableStreamReadRequest) (wire.DurableStreamBatch, *Error)

// Reader is a single-reader source of items. A batch is delivered completely
// before its checkpoint is promoted.
type Reader[T any] struct {
	read    readCall
	mode    wire.DurableStreamMode
	decode  func(payload []byte) ([]T, error)
	request wire.DurableStreamReadRequest
	options ReadOptions
	pending []T
	batch   *wire.DurableStreamBatch
	closed  bool
	failure uint32
	delay   time.Duration
}

// ReadJSON reads a JSON stream, decoding each message as T.
func ReadJSON[T any](url string, options ReadOptions) *Reader[T] {
	return newReader(url, options, wire.DurableStreamModeJson, func(payload []byte) ([]T, error) {
		var items []T
		if len(payload) == 0 {
			return nil, nil
		}
		if err := json.Unmarshal(payload, &items); err != nil {
			return nil, newError(KindProtocolError, "JSON batch decode failed: %v", err)
		}
		return items, nil
	})
}

// ReadBytes reads a byte stream; the items are its bytes, not the original
// append boundaries.
func ReadBytes(url string, options ReadOptions) *Reader[byte] {
	return newReader(url, options, wire.DurableStreamModeBytes, func(payload []byte) ([]byte, error) {
		return payload, nil
	})
}

func newReader[T any](url string, options ReadOptions, mode wire.DurableStreamMode, decode func([]byte) ([]T, error)) *Reader[T] {
	options = options.withDefaults()
	return &Reader[T]{
		read:   hostReader(url, mode, options),
		mode:   mode,
		decode: decode,
		request: wire.DurableStreamReadRequest{
			Checkpoint:  checkpointToWire(options.Checkpoint),
			Transport:   wire.DurableStreamTransportCatchUp,
			ContentType: witTypes.None[string](),
		},
		options: options,
	}
}

// Checkpoint is the last fully delivered position; a partially delivered
// batch keeps its original one.
func (r *Reader[T]) Checkpoint() Checkpoint { return checkpointFromWire(r.request.Checkpoint) }

// Close stops reading and discards buffered items.
func (r *Reader[T]) Close() {
	r.closed = true
	r.pending, r.batch = nil, nil
}

// Next returns the next item. It reports ok=false with a nil error at the end
// of a closed stream; an error never means the end.
func (r *Reader[T]) Next() (T, bool, error) {
	var zero T
	for {
		if r.closed {
			return zero, false, nil
		}
		if r.batch != nil {
			if len(r.pending) > 0 {
				item := r.pending[0]
				r.pending = r.pending[1:]
				return item, true, nil
			}
			batch := r.batch
			r.batch = nil
			r.request.Checkpoint = batch.Next
			r.closed = batch.Closed
			r.request.Transport = wire.DurableStreamTransportCatchUp
			if batch.UpToDate {
				r.request.Transport = wire.DurableStreamTransport(r.options.Transport)
				if len(batch.Payload) == 0 && !batch.Closed {
					r.delay = r.options.IdleDelay
				}
			}
			continue
		}
		if r.delay > 0 {
			time.Sleep(r.delay)
			r.delay = 0
		}
		batch, err := r.read(r.request)
		if err != nil {
			wait, retry := r.options.Retry.delay(err, r.failure)
			if !retry {
				return zero, false, err
			}
			r.failure++
			r.delay = wait
			continue
		}
		if err := r.install(batch); err != nil {
			return zero, false, err
		}
	}
}

func (r *Reader[T]) install(batch wire.DurableStreamBatch) error {
	if batch.Next.Offset == "now" {
		return newError(KindProtocolError, "server did not resolve now")
	}
	items, err := r.decode(batch.Payload)
	if err != nil {
		var se *Error
		if errors.As(err, &se) {
			return se
		}
		return newError(KindProtocolError, "%v", err)
	}
	if r.request.ContentType.IsNone() {
		r.request.ContentType = witTypes.Some(batch.ContentType)
	}
	r.pending, r.batch, r.failure = items, &batch, 0
	return nil
}

// Collect reads every item until the stream is closed.
func (r *Reader[T]) Collect() ([]T, error) {
	var out []T
	for {
		item, ok, err := r.Next()
		if err != nil || !ok {
			return out, err
		}
		out = append(out, item)
	}
}

// AgentStream turns the reader into an agent stream, to return from a method
// or pass to another agent. A read failure fails the invocation, as a failed
// production does.
func (r *Reader[T]) AgentStream() golem.AgentStream[T] {
	return golem.ProduceStream(func(w *golem.AgentStreamWriter[T]) error {
		for {
			item, ok, err := r.Next()
			if err != nil || !ok {
				return err
			}
			if err := w.Write(item); err != nil {
				if errors.Is(err, golem.ErrReaderGone) {
					return nil
				}
				return err
			}
		}
	})
}

func checkpointToWire(c Checkpoint) wire.DurableStreamCheckpoint {
	cursor := witTypes.None[string]()
	if v, ok := c.Cursor.Get(); ok {
		cursor = witTypes.Some(v)
	}
	return wire.DurableStreamCheckpoint{Offset: c.Offset, Cursor: cursor}
}

func checkpointFromWire(c wire.DurableStreamCheckpoint) Checkpoint {
	cursor := golem.None[string]()
	if c.Cursor.IsSome() {
		cursor = golem.Some(c.Cursor.Some())
	}
	return Checkpoint{Offset: c.Offset, Cursor: cursor}
}
