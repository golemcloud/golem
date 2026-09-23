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
	"errors"
	"fmt"
	"iter"
	"reflect"

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
//	        if err := w.Write(line); err != nil { return }
//	    }
//	}()
//	return s
//
// Reading:
//
//	for line, err := range s.All() {
//	    if err != nil { ... }
//	}
//
// A stream endpoint is affine: handing it on transfers it rather than copying
// it, so a stream can be returned, or forwarded to another agent, but not both.
// Forwarding one you have not read costs nothing — the endpoint moves, no items
// are pumped through this component.
//
// Clean completion is end of input. A producer that fails must NOT close the
// stream cleanly, because the reader cannot tell that apart from success; model
// recoverable failures in the item type, as AgentStream[Result[T, E]].

var (
	// ErrStreamTransferred reports a stream used after it was handed on.
	ErrStreamTransferred = errors.New("golem: stream was already transferred")
	// ErrStreamPartiallyRead reports an attempt to forward a stream that has
	// already been read from. The items already taken cannot be put back.
	ErrStreamPartiallyRead = errors.New("golem: stream has already been read and cannot be forwarded")
	// ErrStreamClosed reports a write to a closed stream.
	ErrStreamClosed = errors.New("golem: stream is closed")
	// ErrReaderGone reports that the consumer dropped its end. A producer that
	// sees this should stop; it is cooperative, not an error in the data.
	ErrReaderGone = errors.New("golem: the stream's reader went away")
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
	drop          func()
	// reader is the host endpoint this source wraps, kept so an unread stream
	// can be handed straight back. It is nil for a source that did not come
	// from the host, which therefore cannot be transferred.
	reader any
}

type treeSink struct {
	write         func(items []types.SchemaValueTree) uint32
	readerDropped func() bool
	drop          func()
}

func (s treeSource) valid() bool { return s.read != nil }
func (s treeSink) valid() bool   { return s.write != nil }

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

// NewAgentStreamWithCodec creates a stream whose items are converted by the
// supplied functions, so a caller holding the source schema can pin
// distinctions T alone does not carry. dispose may be nil.
func NewAgentStreamWithCodec[T any](
	encode func(T) (types.SchemaValueTree, error),
	decode func(types.SchemaValueTree) (T, error),
	dispose func(T),
) (*AgentStreamWriter[T], AgentStream[T]) {
	return newAgentStreamWith(streamCodec[T]{encode: encode, decode: decode, dispose: dispose})
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
		defer func() { _ = w.Close() }()
		for _, item := range items {
			if err := w.Write(item); err != nil {
				return
			}
		}
	}()
	return s
}

// ProduceStream runs produce on its own goroutine and returns the stream it
// writes to. Returning nil closes the stream cleanly; returning an error
// abandons it, because a consumer must not read a failed production as a
// complete one.
func ProduceStream[T any](produce func(*AgentStreamWriter[T]) error) AgentStream[T] {
	w, s := NewAgentStream[T]()
	go func() {
		if err := produce(w); err != nil {
			// Abandon rather than close: dropping the writer without a clean
			// close is how the wire distinguishes an incomplete stream.
			w.abandon()
			return
		}
		_ = w.Close()
	}()
	return s
}

// Next reads the next value. It reports ok=false with a nil error at clean end
// of input; a non-nil error is a real failure and never means completion.
func (s AgentStream[T]) Next() (T, bool, error) {
	var zero T
	if s.st == nil {
		return zero, false, ErrStreamClosed
	}
	if s.st.taken {
		return zero, false, ErrStreamTransferred
	}
	if s.st.closed {
		return zero, false, ErrStreamClosed
	}
	s.st.started = true

	tree, ok, err := s.st.nextTree()
	if err != nil || !ok {
		return zero, false, err
	}
	v, err := s.codec.decode(tree)
	if err != nil {
		return zero, false, fmt.Errorf("golem: stream item: %w", err)
	}
	return v, true, nil
}

// nextTree pulls one value tree, refilling the batch when it runs out.
func (st *streamState) nextTree() (types.SchemaValueTree, bool, error) {
	for {
		if len(st.pending) > 0 {
			tree := st.pending[0]
			st.pending = st.pending[1:]
			return tree, true, nil
		}
		if st.done {
			return types.SchemaValueTree{}, false, nil
		}
		if !st.src.valid() {
			return types.SchemaValueTree{}, false, ErrStreamClosed
		}
		batch := make([]types.SchemaValueTree, 1)
		n := st.src.read(batch)
		if n == 0 {
			if st.src.writerDropped() {
				st.done = true
				return types.SchemaValueTree{}, false, nil
			}
			continue
		}
		st.pending = append(st.pending, batch[:n]...)
	}
}

// All iterates the stream. The loop ends at clean end of input; a failure is
// yielded once, with the zero value, and then the iteration stops.
func (s AgentStream[T]) All() iter.Seq2[T, error] {
	return func(yield func(T, error) bool) {
		for {
			v, ok, err := s.Next()
			if err != nil {
				var zero T
				yield(zero, err)
				return
			}
			if !ok {
				return
			}
			if !yield(v, nil) {
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
// write; it does not interrupt work already in flight.
func (s AgentStream[T]) Close() error {
	if s.st == nil || s.st.closed || s.st.taken {
		return nil
	}
	s.st.closed = true
	if s.st.src.valid() {
		s.st.src.drop()
	}
	return nil
}

// Write sends one value. It blocks until the consumer has room, which is what
// applies backpressure to the producer.
func (w *AgentStreamWriter[T]) Write(v T) error {
	if w.closed {
		return ErrStreamClosed
	}
	tree, err := w.codec.encode(v)
	if err != nil {
		if w.codec.dispose != nil {
			// The item was rejected after it was built, so anything it owns has
			// to be released here — nobody else has a reference to it.
			w.codec.dispose(v)
		}
		return fmt.Errorf("golem: stream item: %w", err)
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

// WriteAll sends several values in order, stopping at the first failure.
func (w *AgentStreamWriter[T]) WriteAll(vs ...T) error {
	for _, v := range vs {
		if err := w.Write(v); err != nil {
			return err
		}
	}
	return nil
}

// Close completes the stream. The reader sees end of input.
func (w *AgentStreamWriter[T]) Close() error {
	if w.closed {
		return nil
	}
	w.closed = true
	w.sink.drop()
	return nil
}

// abandon drops the writer without marking completion, which is how a failed
// production is distinguished from a finished one.
func (w *AgentStreamWriter[T]) abandon() {
	if w.closed {
		return
	}
	w.closed = true
	w.sink.drop()
}

// defaultStreamCodec converts items through the SDK's own codec for T.
func defaultStreamCodec[T any]() streamCodec[T] {
	c := defs.compile(reflect.TypeFor[T]())
	return streamCodec[T]{
		encode: func(v T) (types.SchemaValueTree, error) {
			if c.invalid != "" {
				return types.SchemaValueTree{}, errors.New(c.invalid)
			}
			return encodeWith(c, reflect.ValueOf(&v).Elem()), nil
		},
		decode: func(tree types.SchemaValueTree) (T, error) {
			var out T
			if c.invalid != "" {
				return out, errors.New(c.invalid)
			}
			d := decoder{nodes: tree.ValueNodes}
			slot := reflect.ValueOf(&out).Elem()
			if err := c.decode(&d, slot, tree.Root); err != nil {
				return out, err
			}
			return out, nil
		},
	}
}
