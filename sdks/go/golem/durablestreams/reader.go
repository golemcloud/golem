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

// LiveTransport is how a reader waits for new items once it has caught up.
// Reading always starts with catch-up requests.
type LiveTransport uint8

const (
	// LongPoll holds each request open until new items arrive (the default).
	LongPoll LiveTransport = iota
	// SSE receives new items as server-sent events.
	SSE
)

func (t LiveTransport) toWire() wire.DurableStreamTransport {
	if t == SSE {
		return wire.DurableStreamTransportSse
	}
	return wire.DurableStreamTransportLongPoll
}

// ErrorKind classifies a failed read or append. The kinds are the host's
// durable-stream-error-kind cases, in the same order.
type ErrorKind uint8

const (
	InvalidRequest ErrorKind = iota
	PermissionDenied
	NotFound
	Gone
	Closed
	SequenceConflict
	Fenced
	ProducerDiverged
	ProtocolError
	PayloadTooLarge
	Timeout
	Transport
	RateLimited
	Unavailable
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

// DefaultRetry is eight retries from 100ms, doubling up to 30s apart. Options that leave
// Retry unset use it; golem.Some(RetryOptions{}) retries nothing.
var DefaultRetry = RetryOptions{MaxRetries: 8, InitialDelay: 100 * time.Millisecond, MaxDelay: 30 * time.Second}

func (r RetryOptions) delay(err *Error, failures uint32) (time.Duration, bool) {
	switch err.Kind {
	case Timeout, Transport, RateLimited, Unavailable:
	default:
		return 0, false
	}
	if failures >= r.MaxRetries {
		return 0, false
	}
	backoff := backoffAfter(r.InitialDelay, r.MaxDelay, failures)
	if wait, ok := err.RetryAfter.Get(); ok && wait > backoff {
		backoff = wait
	}
	return backoff, true
}

// backoffAfter is initial doubled once per failure, saturating at limit instead
// of overflowing.
func backoffAfter(initial, limit time.Duration, failures uint32) time.Duration {
	backoff := initial
	if backoff <= 0 {
		return 0
	}
	for range failures {
		if backoff >= limit/2 {
			return limit
		}
		backoff *= 2
	}
	return min(backoff, limit)
}

// retryOrDefault is the retry policy an options struct asks for.
func retryOrDefault(retry golem.Option[RetryOptions]) RetryOptions {
	if r, ok := retry.Get(); ok {
		return r
	}
	return DefaultRetry
}

// ReadOptions configures a Reader; the zero value reads from [Start] with long
// polling, a 30s attempt deadline and [DefaultRetry].
type ReadOptions struct {
	Checkpoint Checkpoint
	// Live is how the reader waits once it has caught up.
	Live    LiveTransport
	Timeout time.Duration
	// IdleDelay is the pause after an empty batch at the tail.
	IdleDelay time.Duration
	// Retry bounds the retries; unset means [DefaultRetry].
	Retry golem.Option[RetryOptions]
	// Auth authenticates the requests; its plaintext never reaches the agent.
	Auth golem.Option[golem.Secret[string]]
}

func (o ReadOptions) withDefaults() ReadOptions {
	if o.Checkpoint.Offset == "" {
		o.Checkpoint = Start
	}
	if o.Timeout == 0 {
		o.Timeout = 30 * time.Second
	}
	if o.IdleDelay == 0 {
		o.IdleDelay = 100 * time.Millisecond
	}
	o.Retry = golem.Some(retryOrDefault(o.Retry))
	return o
}

// readCall performs one read attempt; the wasm build binds it to the host
// reader (durablestreams_wasm.go).
type readCall func(wire.DurableStreamReadRequest) (wire.DurableStreamBatch, *Error)

// Reader is a single-reader source of items. A batch is delivered completely
// before its checkpoint is promoted.
type Reader[T any] struct {
	read    readCall
	release func()
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
			return nil, newError(ProtocolError, "JSON batch decode failed: %v", err)
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
	read, release := hostReader(url, mode, options)
	return &Reader[T]{
		read:    read,
		release: release,
		mode:    mode,
		decode:  decode,
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

// Close stops reading, discards buffered items and releases the reader's host
// resource. A reader that is not closed is released when it is garbage
// collected.
func (r *Reader[T]) Close() {
	r.closed = true
	r.pending, r.batch = nil, nil
	if r.release != nil {
		r.release()
		r.release = nil
	}
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
				r.request.Transport = r.options.Live.toWire()
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
			wait, retry := retryOrDefault(r.options.Retry).delay(err, r.failure)
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
		return newError(ProtocolError, "server did not resolve now")
	}
	items, err := r.decode(batch.Payload)
	if err != nil {
		var se *Error
		if errors.As(err, &se) {
			return se
		}
		return newError(ProtocolError, "%v", err)
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
