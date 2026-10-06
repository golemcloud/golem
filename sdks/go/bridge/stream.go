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

package bridge

import (
	"context"
	"errors"
	"fmt"
	"iter"
	"slices"
	"sync"

	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// Agent streams.
//
// An AgentStream is a sequence of values passed to or returned by a streaming
// agent method. Values flow while the method runs, over the invocation session
// the call opened:
//
//	out, err := client.Transform(ctx, bridge.StreamOf[uint32](1, 2, 3))
//	for v, err := range out.All(ctx) { … }
//
// A stream is read once. Handing one to a method gives it away; reading it
// afterwards, or handing it on twice, fails with ErrStreamConsumed.
//
// A stream returned by a method ends with nil at clean completion, with a
// *StreamError when the producer failed, or with a *StreamCancelled when either
// side cancelled it. Close a returned stream you stop reading early: the agent
// learns that nobody is listening, and the session can finish.

// ErrStreamConsumed reports a stream read or passed on after it was already
// given away, or passed twice.
var ErrStreamConsumed = errors.New("golem: the stream was already consumed")

// ErrStreamClosed reports a write to a stream whose reader has gone.
var ErrStreamClosed = errors.New("golem: the stream's reader closed it")

// StreamError is the failure that ended a stream returned by a method.
type StreamError struct {
	Code    string
	Message string
}

func (e *StreamError) Error() string {
	return fmt.Sprintf("golem: stream failed (%s): %s", e.Code, e.Message)
}

// StreamCancelled reports a stream that was cancelled before it completed.
type StreamCancelled struct {
	Reason string
}

func (e *StreamCancelled) Error() string {
	return fmt.Sprintf("golem: stream cancelled (%s)", e.Reason)
}

// AgentStream is a sequence of values streamed to or from an agent method.
type AgentStream[T any] struct {
	core *streamCore[T]
}

type streamUse uint8

const (
	streamFresh streamUse = iota
	streamReading
	streamGiven
)

type streamCore[T any] struct {
	mu  sync.Mutex
	use streamUse
	src source[T]
}

// source is where a stream's values come from: values held in this process,
// or an invocation session receiving them.
type source[T any] interface {
	next(ctx context.Context) (T, bool, error)
	// ready reports whether next would return without waiting.
	ready() bool
	// stop tells the producer that nobody reads any more.
	stop(reason string)
}

func newStream[T any](src source[T]) AgentStream[T] {
	return AgentStream[T]{core: &streamCore[T]{src: src}}
}

func (s AgentStream[T]) claim(use streamUse) (source[T], error) {
	if s.core == nil {
		return nil, errors.New("golem: the zero AgentStream is not a stream")
	}
	s.core.mu.Lock()
	defer s.core.mu.Unlock()
	switch {
	case s.core.use == streamGiven:
		return nil, ErrStreamConsumed
	case use == streamGiven && s.core.use != streamFresh:
		return nil, ErrStreamConsumed
	}
	s.core.use = use
	return s.core.src, nil
}

// Next reads the next value. It reports ok=false with a nil error at clean end
// of the stream.
func (s AgentStream[T]) Next(ctx context.Context) (T, bool, error) {
	src, err := s.claim(streamReading)
	if err != nil {
		var zero T
		return zero, false, err
	}
	return src.next(ctx)
}

// All iterates the stream. The loop ends at clean end; a failure is yielded as
// the final error.
func (s AgentStream[T]) All(ctx context.Context) iter.Seq2[T, error] {
	return func(yield func(T, error) bool) {
		for {
			v, ok, err := s.Next(ctx)
			if err != nil {
				var zero T
				yield(zero, err)
				return
			}
			if !ok || !yield(v, nil) {
				return
			}
		}
	}
}

// Collect reads the stream to completion.
func (s AgentStream[T]) Collect(ctx context.Context) ([]T, error) {
	var out []T
	for v, err := range s.All(ctx) {
		if err != nil {
			return out, err
		}
		out = append(out, v)
	}
	return out, nil
}

// Close stops reading. A stream returned by a method tells the agent nobody
// listens any more; a stream made here stops its producer.
func (s AgentStream[T]) Close() error {
	src, err := s.claim(streamReading)
	if err != nil {
		return err
	}
	src.stop("consumer-drop")
	return nil
}

// StreamOf is a stream of known values. The values are copied, so the caller
// keeps its slice.
func StreamOf[T any](items ...T) AgentStream[T] {
	return newStream[T](&sliceSource[T]{items: slices.Clone(items)})
}

type sliceSource[T any] struct {
	mu    sync.Mutex
	items []T
}

func (s *sliceSource[T]) next(context.Context) (T, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var zero T
	if len(s.items) == 0 {
		return zero, false, nil
	}
	v := s.items[0]
	s.items[0] = zero
	s.items = s.items[1:]
	return v, true, nil
}

func (s *sliceSource[T]) ready() bool { return true }

func (s *sliceSource[T]) stop(string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.items = nil
}

// AgentStreamWriter produces the values of a stream made with NewAgentStream.
type AgentStreamWriter[T any] struct {
	items   chan T
	ended   chan struct{}
	stopped chan struct{}
	once    sync.Once
	stopOne sync.Once
	err     error
}

// streamBuffer is how many written values wait for the reader before Write
// blocks.
const streamBuffer = 256

// NewAgentStream creates a stream and the writer that produces it. The writer
// may be used from any goroutine; Close or CloseWithError ends the stream.
func NewAgentStream[T any]() (*AgentStreamWriter[T], AgentStream[T]) {
	w := &AgentStreamWriter[T]{
		items:   make(chan T, streamBuffer),
		ended:   make(chan struct{}),
		stopped: make(chan struct{}),
	}
	return w, newStream[T](&writerSource[T]{w: w})
}

// ProduceStream runs produce on its own goroutine and returns the stream it
// writes. Returning nil ends the stream cleanly; an error fails it, which the
// agent sees as its input becoming unavailable.
func ProduceStream[T any](produce func(*AgentStreamWriter[T]) error) AgentStream[T] {
	w, s := NewAgentStream[T]()
	go func() {
		if err := produce(w); err != nil {
			_ = w.CloseWithError(err)
			return
		}
		_ = w.Close()
	}()
	return s
}

// Write sends one value. It blocks while the reader is behind, and fails with
// ErrStreamClosed once the reader has gone.
func (w *AgentStreamWriter[T]) Write(v T) error {
	select {
	case <-w.stopped:
		return ErrStreamClosed
	case <-w.ended:
		return errors.New("golem: write to a closed stream")
	default:
	}
	select {
	case w.items <- v:
		return nil
	case <-w.stopped:
		return ErrStreamClosed
	}
}

// WriteAll sends several values in order, stopping at the first failure.
func (w *AgentStreamWriter[T]) WriteAll(vs ...T) error {
	for _, v := range vs {
		if err := w.Write(v); err != nil {
			return err
		}
	}
	return nil
}

// Close ends the stream cleanly once the values already written are read.
func (w *AgentStreamWriter[T]) Close() error {
	w.once.Do(func() { close(w.ended) })
	return nil
}

// CloseWithError ends the stream with a failure.
func (w *AgentStreamWriter[T]) CloseWithError(err error) error {
	w.once.Do(func() {
		w.err = err
		close(w.ended)
	})
	return nil
}

type writerSource[T any] struct{ w *AgentStreamWriter[T] }

func (s *writerSource[T]) next(ctx context.Context) (T, bool, error) {
	var zero T
	select {
	case v := <-s.w.items:
		return v, true, nil
	default:
	}
	select {
	case v := <-s.w.items:
		return v, true, nil
	case <-s.w.ended:
		// Values written before Close are still read.
		select {
		case v := <-s.w.items:
			return v, true, nil
		default:
		}
		if s.w.err != nil {
			return zero, false, s.w.err
		}
		return zero, false, nil
	case <-s.w.stopped:
		return zero, false, ErrStreamClosed
	case <-ctx.Done():
		return zero, false, ctx.Err()
	}
}

func (s *writerSource[T]) ready() bool {
	if len(s.w.items) > 0 {
		return true
	}
	select {
	case <-s.w.ended:
		return true
	default:
		return false
	}
}

func (s *writerSource[T]) stop(string) {
	s.w.stopOne.Do(func() { close(s.w.stopped) })
}

// Generated clients pass streams through these.

// lane is how a stream's items travel: packed bytes for stream<u8>, one binary
// frame per item for stream<binary>, JSON for everything else.
type lane uint8

const (
	laneJSON lane = iota
	laneU8
	laneBinary
)

func (l lane) String() string {
	switch l {
	case laneU8:
		return "u8"
	case laneBinary:
		return "binary"
	}
	return "json"
}

func laneOf[T any]() lane {
	var zero T
	switch any(zero).(type) {
	case uint8:
		return laneU8
	case values.Binary:
		return laneBinary
	}
	return laneJSON
}

// inputHandle is a stream argument on its way into a session: its items as
// schema values, and the lane they travel on.
type inputHandle struct {
	lane  lane
	next  func(ctx context.Context) (schema.SchemaValue, bool, error)
	ready func() bool
	stop  func(reason string)
	// err is set when the stream could not be given away; the session
	// reports it instead of sending the call.
	err error
}

// Release does nothing: an input is released by the session that read it.
func (*inputHandle) Release() {}

// EncodeStream is the encoder of a stream argument. The stream is given to the
// session that sends the call, which reads it and encodes each value with item.
func EncodeStream[T any](item Encoder[T]) Encoder[AgentStream[T]] {
	return func(s AgentStream[T]) schema.SchemaValue {
		src, err := s.claim(streamGiven)
		if err != nil {
			return schema.StreamValue{Handle: &inputHandle{err: err}}
		}
		return schema.StreamValue{Handle: &inputHandle{
			lane: laneOf[T](),
			next: func(ctx context.Context) (schema.SchemaValue, bool, error) {
				v, ok, err := src.next(ctx)
				if err != nil || !ok {
					return nil, ok, err
				}
				return item(v), true, nil
			},
			ready: src.ready,
			stop:  src.stop,
		}}
	}
}

// DecodeStream is the decoder of a stream in a result or stream item. The
// stream reads from the session that received it, decoding each value with
// item.
func DecodeStream[T any](item Decoder[T]) Decoder[AgentStream[T]] {
	return func(sv schema.SchemaValue) (AgentStream[T], error) {
		v, ok := sv.(schema.StreamValue)
		if !ok {
			return AgentStream[T]{}, Mismatch("stream", sv)
		}
		out, ok := v.Handle.(*outputStream)
		if !ok {
			return AgentStream[T]{}, errors.New("golem: a stream can only be read from the invocation session that returned it")
		}
		if err := out.expose(); err != nil {
			return AgentStream[T]{}, err
		}
		return newStream[T](&remoteSource[T]{out: out, item: item}), nil
	}
}

type remoteSource[T any] struct {
	out  *outputStream
	item Decoder[T]
}

func (s *remoteSource[T]) next(ctx context.Context) (T, bool, error) {
	var zero T
	sv, ok, err := s.out.next(ctx)
	if err != nil || !ok {
		return zero, ok, err
	}
	v, err := s.item(sv)
	if err != nil {
		err = fmt.Errorf("golem: decoding a stream item: %w", err)
		s.out.s.fail(err)
		return zero, false, err
	}
	return v, true, nil
}

func (s *remoteSource[T]) ready() bool { return s.out.ready() }

func (s *remoteSource[T]) stop(reason string) { s.out.stop(reason) }
