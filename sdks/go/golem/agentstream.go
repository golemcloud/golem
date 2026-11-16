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

package golem

import (
	"context"
	"errors"
	"fmt"
	"io"
	"iter"
	"reflect"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Agent streams.
//
// An [AgentStream] carries a sequence of schema values between agents. It is
// the type a streaming method takes or returns:
//
//	var Tail = Logs.Method[TailIn, golem.AgentStream[LogLine]]("tail")
//
//	w, s := golem.NewAgentStream[LogLine]()
//	go func() {
//	    defer w.Close()              // clean completion; the reader sees EOF
//	    for _, line := range lines { // Write blocks: that is the backpressure
//	        if err := w.Write(line); err != nil { return } // the reader went away
//	    }
//	}()
//	return s
//
// Reading:
//
//	for line, err := range s.All() {
//	    if err != nil { return err }
//	    ...
//	}
//
// Reading a stream after it was handed on or closed, writing to a closed
// writer, and an item that does not match its schema are bugs, not conditions
// a caller can recover from, so they panic.
//
// A stream endpoint is affine: handing it on transfers it rather than copying
// it, so a stream can be returned, or forwarded to another agent, but not both.
// Forwarding one you have not read costs nothing — the endpoint moves, no items
// are pumped through this component.
//
// A producer failure follows its accepted items as a StreamError. Per-item
// Result values remain ordinary items, independent of the stream outcome.

// StreamErrorKind identifies the producer's terminal failure.
type StreamErrorKind uint8

const (
	StreamFailed StreamErrorKind = iota
	StreamTrapped
	StreamCancelled
	StreamAbandoned
	StreamInterrupted
	StreamResourceExhausted
)

// StreamError carries a producer outcome. Value is present for StreamFailed;
// Message carries diagnostics for traps, interruptions and resource limits.
type StreamError struct {
	Kind    StreamErrorKind
	Value   TypedValue
	Message string
}

func (e *StreamError) Error() string {
	if e.Message != "" {
		return e.Message
	}
	return fmt.Sprintf("golem: stream %s", [...]string{"failed", "trapped", "cancelled", "abandoned", "interrupted", "resource-exhausted"}[e.Kind])
}

func (e *StreamError) Unwrap() error {
	if e.Kind == StreamCancelled {
		return context.Canceled
	}
	return nil
}

func streamErrorFromWire(e types.StreamError) error {
	switch e.Tag() {
	case types.StreamErrorFailed:
		return &StreamError{Kind: StreamFailed, Value: TypedValue{wit: e.Failed()}}
	case types.StreamErrorTrapped:
		return &StreamError{Kind: StreamTrapped, Message: e.Trapped()}
	case types.StreamErrorCancelled:
		return &StreamError{Kind: StreamCancelled}
	case types.StreamErrorAbandoned:
		return &StreamError{Kind: StreamAbandoned}
	case types.StreamErrorInterrupted:
		return &StreamError{Kind: StreamInterrupted, Message: e.Interrupted()}
	case types.StreamErrorResourceExhausted:
		return &StreamError{Kind: StreamResourceExhausted, Message: e.ResourceExhausted()}
	default:
		panic("golem: invalid stream outcome")
	}
}

func streamFailure(err error) types.StreamError {
	var declared *StreamError
	if errors.As(err, &declared) && declared.Kind == StreamFailed {
		return types.MakeStreamErrorFailed(declared.Value.wit)
	}
	return types.MakeStreamErrorFailed(EncodeTypedValue(err.Error()).wit)
}

// ErrReaderGone reports that the consumer dropped its end. A producer that
// sees this should stop; it is cooperative, not an error in the data.
var ErrReaderGone = errors.New("golem: the stream's reader went away")

var (
	// errStreamTransferred reports a stream used after it was handed on.
	errStreamTransferred = errors.New("golem: stream was already transferred")
	// errStreamPartiallyRead reports an attempt to forward a stream that has
	// already been read from. The items already taken cannot be put back.
	errStreamPartiallyRead = errors.New("golem: stream has already been read and cannot be forwarded")
	// errStreamClosed reports a use of a closed stream.
	errStreamClosed = errors.New("golem: stream is closed")
)

// treeSource is the reading half of a schema-value stream, and treeSink the
// writing half. They are structs of functions rather than interfaces on
// purpose: an interface call here builds an itab, and the linker then retains
// the method sets of every structurally-matching type — which off the wasm
// target drags host-call methods into a link that has no bodies for them. The
// indirection still keeps the chunking, EOF and ownership logic testable
// without a host.
type treeSource struct {
	read          func(dst []types.SchemaValueTree) uint32
	writerDropped func() bool
	outcome       func() error
	//nolint:unused // released by the wasip1 build when forwarding an unread stream
	dropOutcome func()
	drop        func()
	// reader is the host endpoint this source wraps, kept so an unread stream
	// can be handed straight back. It is nil for a source that did not come
	// from the host, which therefore cannot be transferred.
	//nolint:unused // read by the wasip1 build, to hand an unread stream back
	reader any
	//nolint:unused // transferred by the wasip1 build
	future any
}

type treeSink struct {
	write         func(items []types.SchemaValueTree) uint32
	readerDropped func() bool
	drop          func()
	finish        func(error)
}

func (s treeSource) valid() bool { return s.read != nil }

// streamCodec converts between items and the value trees on the wire. It is
// derived from the SOURCE schema rather than from T alone, because a Go type
// does not determine its schema: []T is a list or a fixed list, string is a
// string or text or a path, int64 is an s64 or a quantity mantissa.
type streamCodec[T any] struct {
	encode func(T) (types.SchemaValueTree, error)
	decode func(types.SchemaValueTree) (T, error)
	// dispose releases endpoints an item still owns, for the case where the
	// item is rejected after it was built.
	dispose func(T)
}

// streamState is the shared state behind an [AgentStream]. Copies of the stream
// alias it, so a transfer made through one copy is seen by all of them — which
// is what makes the endpoint affine rather than merely conventionally so.
type streamState struct {
	src treeSource
	// pending holds the remainder of a read batch.
	pending []types.SchemaValueTree
	started bool
	taken   bool
	done    bool
	closed  bool
}

// AgentStream is a sequence of values streamed between agents.
type AgentStream[T any] struct {
	st    *streamState
	codec streamCodec[T]
}

// AgentStreamWriter produces the values of an [AgentStream].
type AgentStreamWriter[T any] struct {
	sink   treeSink
	codec  streamCodec[T]
	closed bool
}

// NewAgentStream creates a stream and its writer, with item conversion derived
// from T.
func NewAgentStream[T any]() (*AgentStreamWriter[T], AgentStream[T]) {
	return newAgentStreamWith(defaultStreamCodec[T]())
}

func newAgentStreamWith[T any](c streamCodec[T]) (*AgentStreamWriter[T], AgentStream[T]) {
	sink, src := newStreamPair()
	return &AgentStreamWriter[T]{sink: sink, codec: c},
		AgentStream[T]{st: &streamState{src: src}, codec: c}
}

// StreamOf is a stream of known values, produced without a goroutine. It is
// mostly useful in tests and for small fixed results.
func StreamOf[T any](items ...T) AgentStream[T] {
	w, s := NewAgentStream[T]()
	go func() {
		defer w.Close()
		for _, item := range items {
			if err := w.Write(item); err != nil {
				return
			}
		}
	}()
	return s
}

// ProduceStream runs produce on its own goroutine and returns the stream it
// writes to. Returning nil closes the stream cleanly, and so does returning
// [ErrReaderGone] (or an error wrapping it), which only says the consumer
// stopped reading. Another error produces a failed outcome. A StreamError
// with Kind StreamFailed preserves its declared Value; other errors carry a string.
func ProduceStream[T any](produce func(*AgentStreamWriter[T]) error) AgentStream[T] {
	w, s := NewAgentStream[T]()
	go func() {
		err := produce(w)
		if errors.Is(err, ErrReaderGone) {
			err = nil
		}
		w.Finish(err)
	}()
	return s
}

// Next reads the next value, reporting io.EOF at clean end of input. It
// panics on a stream that was handed on or closed, and on an item that does
// not match the stream's schema.
func (s AgentStream[T]) Next() (T, error) {
	var zero T
	if s.st == nil {
		panic(errStreamClosed)
	}
	if s.st.taken {
		panic(errStreamTransferred)
	}
	if s.st.closed {
		panic(errStreamClosed)
	}
	s.st.started = true

	tree, ok := s.st.nextTree()
	if !ok {
		if s.st.src.outcome != nil {
			outcome := s.st.src.outcome
			s.st.src.outcome = nil
			if err := outcome(); err != nil {
				return zero, err
			}
		}
		return zero, io.EOF
	}
	v, err := s.codec.decode(tree)
	if err != nil {
		panic(fmt.Errorf("golem: stream item: %w", err))
	}
	return v, nil
}

// nextTree pulls one value tree, refilling the batch when it runs out.
func (st *streamState) nextTree() (types.SchemaValueTree, bool) {
	for {
		if len(st.pending) > 0 {
			tree := st.pending[0]
			st.pending = st.pending[1:]
			return tree, true
		}
		if st.done {
			return types.SchemaValueTree{}, false
		}
		if !st.src.valid() {
			panic(errStreamClosed)
		}
		batch := make([]types.SchemaValueTree, 1)
		n := st.src.read(batch)
		if n == 0 {
			if st.src.writerDropped() {
				st.done = true
				return types.SchemaValueTree{}, false
			}
			continue
		}
		st.pending = append(st.pending, batch[:n]...)
	}
}

// All yields each item and then at most one terminal error. Stopping early
// releases the reader without awaiting the producer outcome.
func (s AgentStream[T]) All() iter.Seq2[T, error] {
	return func(yield func(T, error) bool) {
		defer s.Close()
		for {
			v, err := s.Next()
			if errors.Is(err, io.EOF) {
				return
			}
			if !yield(v, err) || err != nil {
				return
			}
		}
	}
}

// Collect reads the stream to completion. Use it only where the sequence is
// known to be bounded — it defeats the point of streaming otherwise.
func (s AgentStream[T]) Collect() ([]T, error) {
	var out []T
	for v, err := range s.All() {
		if err != nil {
			return out, err
		}
		out = append(out, v)
	}
	return out, nil
}

// Close releases the reading endpoint. A producer observes it on its next
// write; it does not interrupt work already in flight. Closing a closed or
// handed-on stream does nothing.
func (s AgentStream[T]) Close() {
	if s.st == nil || s.st.closed || s.st.taken {
		return
	}
	s.st.closed = true
	if s.st.src.valid() {
		s.st.src.drop()
	}
}

// Write sends one value. It blocks until the consumer has room, which is what
// applies backpressure to the producer. The only error is [ErrReaderGone]; a
// write to a closed writer, or of a value that cannot be encoded, panics.
func (w *AgentStreamWriter[T]) Write(v T) error {
	if w.closed {
		panic(errStreamClosed)
	}
	tree, err := w.codec.encode(v)
	if err != nil {
		if w.codec.dispose != nil {
			// The item was rejected after it was built, so anything it owns has
			// to be released here — nobody else has a reference to it.
			w.codec.dispose(v)
		}
		panic(fmt.Errorf("golem: stream item: %w", err))
	}
	batch := []types.SchemaValueTree{tree}
	for len(batch) > 0 {
		if w.sink.readerDropped() {
			return ErrReaderGone
		}
		n := w.sink.write(batch)
		batch = batch[n:]
	}
	return nil
}

// WriteAll sends several values in order, stopping when the reader goes away.
func (w *AgentStreamWriter[T]) WriteAll(vs ...T) error {
	for _, v := range vs {
		if err := w.Write(v); err != nil {
			return err
		}
	}
	return nil
}

// Close completes the stream. The reader sees end of input. Closing twice does
// nothing, so a deferred Close after an explicit one is safe.
func (w *AgentStreamWriter[T]) Close() {
	w.Finish(nil)
}

// Finish closes the item stream and reports its outcome. A failed declared
// value is supplied as &StreamError{Kind: StreamFailed, Value: value}.
func (w *AgentStreamWriter[T]) Finish(err error) {
	if w.closed {
		return
	}
	w.closed = true
	if w.sink.finish != nil {
		w.sink.finish(err)
	} else {
		w.sink.drop()
	}
}

// defaultStreamCodec converts items through the SDK's own codec for T.
func defaultStreamCodec[T any]() streamCodec[T] {
	c := defs.Compile(reflect.TypeFor[T]())
	return streamCodec[T]{
		encode: func(v T) (types.SchemaValueTree, error) {
			if c.Invalid != "" {
				return types.SchemaValueTree{}, errors.New(c.Invalid)
			}
			return engine.EncodeWith(c, reflect.ValueOf(&v).Elem()), nil
		},
		decode: func(tree types.SchemaValueTree) (T, error) {
			var out T
			d := engine.Decoder{Nodes: tree.ValueNodes}
			if c.Invalid != "" {
				d.ReleaseCards(releaseCard)
				return out, errors.New(c.Invalid)
			}
			slot := reflect.ValueOf(&out).Elem()
			if err := c.Decode(&d, slot, tree.Root); err != nil {
				d.ReleaseCards(releaseCard)
				var zero T
				return zero, err
			}
			d.ReleaseRest(releaseCard)
			return out, nil
		},
	}
}
