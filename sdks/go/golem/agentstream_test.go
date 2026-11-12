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
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"io"
	"reflect"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// fakeStream is an in-memory endpoint pair standing in for the host's.
type fakeStream struct {
	items    []types.SchemaValueTree
	at       int
	dropped  bool // the writer finished or abandoned
	rdropped bool // the reader went away
	// accept caps how many items a write takes, so backpressure can be
	// exercised: the host may accept fewer than offered.
	accept int
}

func (f *fakeStream) source() treeSource {
	return treeSource{
		read: func(dst []types.SchemaValueTree) uint32 {
			if f.at >= len(f.items) {
				return 0
			}
			n := copy(dst, f.items[f.at:])
			f.at += n
			return uint32(n)
		},
		writerDropped: func() bool { return f.dropped && f.at >= len(f.items) },
		drop:          func() { f.dropped = true },
	}
}

func (f *fakeStream) sink() treeSink {
	return treeSink{
		write: func(items []types.SchemaValueTree) uint32 {
			if f.rdropped {
				return 0
			}
			n := len(items)
			if f.accept > 0 && f.accept < n {
				n = f.accept
			}
			f.items = append(f.items, items[:n]...)
			return uint32(n)
		},
		readerDropped: func() bool { return f.rdropped },
		drop:          func() { f.dropped = true },
	}
}

// streamOverFake builds a stream reading from a scripted endpoint.
func streamOverFake[T any](t *testing.T, values ...T) (AgentStream[T], *fakeStream) {
	t.Helper()
	c := defaultStreamCodec[T]()
	f := &fakeStream{dropped: true}
	for _, v := range values {
		tree, err := c.encode(v)
		if err != nil {
			t.Fatalf("encoding %v: %v", v, err)
		}
		f.items = append(f.items, tree)
	}
	return AgentStream[T]{st: &streamState{src: f.source()}, codec: c}, f
}

func TestAgentStreamReadsToCleanEOF(t *testing.T) {
	s, _ := streamOverFake(t, "a", "b", "c")
	got, err := s.Collect()
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, []string{"a", "b", "c"}) {
		t.Errorf("read %v", got)
	}
}

// A clean end of input is io.EOF, and reading past it stays there.
func TestAgentStreamEOF(t *testing.T) {
	s, _ := streamOverFake(t, "only")
	if _, err := s.Next(); err != nil {
		t.Fatal("first read reported end of input")
	}
	for range 2 {
		if v, err := s.Next(); !errors.Is(err, io.EOF) {
			t.Errorf("end of input reported as err=%v value=%v", err, v)
		}
	}
}

func TestAgentStreamAllStopsAtEOF(t *testing.T) {
	s, _ := streamOverFake(t, int64(1), int64(2))
	var seen []int64
	for v, err := range s.All() {
		if err != nil {
			t.Fatal(err)
		}
		seen = append(seen, v)
	}
	if !reflect.DeepEqual(seen, []int64{1, 2}) {
		t.Errorf("iterated %v", seen)
	}
}

// TestAgentStreamWriteAppliesBackpressure — the host may accept fewer items
// than offered; Write must keep going rather than silently dropping them.
func TestAgentStreamWriteAppliesBackpressure(t *testing.T) {
	f := &fakeStream{accept: 1}
	w := &AgentStreamWriter[string]{sink: f.sink(), codec: defaultStreamCodec[string]()}
	if err := w.WriteAll("a", "b", "c"); err != nil {
		t.Fatalf("WriteAll: %v", err)
	}
	if len(f.items) != 3 {
		t.Errorf("sink received %d items, want 3", len(f.items))
	}
}

// TestAgentStreamWriteStopsWhenTheReaderGoesAway — cooperative, observed on a
// write, and reported as a value so a producer can stop cleanly.
func TestAgentStreamWriteStopsWhenTheReaderGoesAway(t *testing.T) {
	f := &fakeStream{rdropped: true}
	w := &AgentStreamWriter[string]{sink: f.sink(), codec: defaultStreamCodec[string]()}
	if err := w.Write("a"); !errors.Is(err, ErrReaderGone) {
		t.Errorf("write to a dropped reader gave %v, want ErrReaderGone", err)
	}
}

func TestAgentStreamCloseIsCleanCompletion(t *testing.T) {
	f := &fakeStream{}
	w := &AgentStreamWriter[string]{sink: f.sink(), codec: defaultStreamCodec[string]()}
	_ = w.Write("a")
	w.Close()
	if !f.dropped {
		t.Error("closing the writer did not complete the stream")
	}
	// Closing twice does nothing; a deferred Close after an explicit one is
	// the normal Go shape.
	w.Close()
	mustPanic(t, "stream is closed", func() { _ = w.Write("b") })
}

// TestAnUnencodableItemPanics — an item the stream's codec cannot encode is a
// bug in the producer, not a condition it could handle.
func TestAnUnencodableItemPanics(t *testing.T) {
	f := &fakeStream{}
	w := &AgentStreamWriter[chan int]{sink: f.sink(), codec: defaultStreamCodec[chan int]()}
	mustPanic(t, "stream item", func() { _ = w.Write(make(chan int)) })
}

// TestReadingAClosedStreamPanics — Close releases the endpoint, so a later
// read is a bug; closing again does nothing.
func TestReadingAClosedStreamPanics(t *testing.T) {
	s, _ := streamOverFake(t, "a")
	s.Close()
	s.Close()
	mustPanic(t, "stream is closed", func() { s.Next() })
}

// TestAgentStreamTransferIsAffine — handing a stream on moves the endpoint;
// aliases share the state, so a second transfer through any copy is refused.
func TestAgentStreamTransferIsAffine(t *testing.T) {
	s, _ := streamOverFake(t, "a")
	alias := s

	if _, err := s.streamTake(); err != nil {
		t.Fatalf("first transfer: %v", err)
	}
	if _, err := alias.streamTake(); !errors.Is(err, errStreamTransferred) {
		t.Errorf("second transfer through an alias gave %v, want errStreamTransferred", err)
	}
	mustPanic(t, "already transferred", func() { s.Next() })
}

// TestAgentStreamPartiallyReadCannotBeForwarded — the items already taken
// cannot be put back, so the receiver would not get the stream it was promised.
func TestAgentStreamPartiallyReadCannotBeForwarded(t *testing.T) {
	s, _ := streamOverFake(t, "a", "b")
	if _, err := s.Next(); err != nil {
		t.Fatal("first read reported end of input")
	}
	if _, err := s.streamTake(); !errors.Is(err, errStreamPartiallyRead) {
		t.Errorf("forwarding a partially read stream gave %v", err)
	}
}

// TestAgentStreamForwardingUnreadDoesNotPump — forwarding is a pure endpoint
// move: nothing is read out of the source on the way through.
func TestAgentStreamForwardingUnreadDoesNotPump(t *testing.T) {
	s, f := streamOverFake(t, "a", "b", "c")
	if _, err := s.streamTake(); err != nil {
		t.Fatalf("transfer: %v", err)
	}
	if f.at != 0 {
		t.Errorf("forwarding consumed %d items; it must move the endpoint, not pump it", f.at)
	}
}

// TestAgentStreamDecodeErrorIsNotEOF — a malformed item must not look like the
// end of the stream, or a consumer silently truncates: it panics instead.
func TestAgentStreamDecodeErrorIsNotEOF(t *testing.T) {
	var b engine.ValBuilder
	root := b.Push(types.MakeSchemaValueNodeStringValue("not an int"))
	f := &fakeStream{
		dropped: true,
		items:   []types.SchemaValueTree{{ValueNodes: b.Nodes, Root: root}},
	}
	s := AgentStream[int64]{st: &streamState{src: f.source()}, codec: defaultStreamCodec[int64]()}

	mustPanic(t, "stream item", func() { s.Next() })
}

// TestStreamSchemaIsAlwaysTyped — an untyped stream cannot supply an item
// codec, and Go has no way to spell one.
func TestStreamSchemaIsAlwaysTyped(t *testing.T) {
	g := engine.GraphBuilder{E: defs.Engine}
	root := g.Node(defs.Compile(reflect.TypeFor[AgentStream[string]]()))
	body := g.Build().TypeNodes[root].Body
	if body.Tag() != types.SchemaTypeBodyStreamType {
		t.Fatalf("AgentStream lowered to tag %d, want stream-type", body.Tag())
	}
	item := body.StreamType()
	if item.IsNone() {
		t.Fatal("the stream carries no item type")
	}
	if tag := g.Build().TypeNodes[item.Some()].Body.Tag(); tag != types.SchemaTypeBodyStringType {
		t.Errorf("item type tag %d, want string", tag)
	}
}

// TestCarriesStreamFindsNestedStreams — a stream buried in a record or reached
// only through a recursive type still rules out Trigger.
func TestCarriesStreamFindsNestedStreams(t *testing.T) {
	type Inner struct{ Lines AgentStream[string] }
	type Outer struct {
		Name  string
		Inner Inner
	}
	for _, tc := range []struct {
		name string
		rt   reflect.Type
		want bool
	}{
		{"the stream itself", reflect.TypeFor[AgentStream[string]](), true},
		{"a record holding one", reflect.TypeFor[Inner](), true},
		{"a record holding that", reflect.TypeFor[Outer](), true},
		{"an option of one", reflect.TypeFor[Option[AgentStream[string]]](), true},
		{"a list of them", reflect.TypeFor[[]AgentStream[string]](), true},
		{"a plain record", reflect.TypeFor[struct{ Name string }](), false},
		{"a recursive type holding one", reflect.TypeFor[streamTree](), true},
	} {
		d := newDefinitions()
		if got := d.CarriesStream(tc.rt); got != tc.want {
			t.Errorf("%s: carriesStream=%v, want %v", tc.name, got, tc.want)
		}
	}
}

// A received stream lands in a field of the method's input struct, which the
// decoder fills through reflection. Reflection cannot set the stream's
// unexported state, so the decoder initialises a fresh value through a method
// instead; this is that exact path, for a stream held in a struct field.
func TestAReceivedStreamCanBeStoredInAStructField(t *testing.T) {
	type in struct {
		Chunks AgentStream[byte]
		Name   string
	}
	var v in
	dst := reflect.ValueOf(&v).Elem().Field(0)
	fresh := reflect.New(dst.Type())
	fresh.Interface().(streamReceiver).streamReceive(treeSource{})
	dst.Set(fresh.Elem())
	if v.Chunks.st == nil {
		t.Fatal("the received stream has no state")
	}
	if v.Chunks.codec.decode == nil {
		t.Fatal("the received stream cannot decode its items")
	}
}

// A producer failure follows the accepted prefix without trapping the reader.
func TestAFailedProductionIsNotEndOfInput(t *testing.T) {
	s := ProduceStream(func(w *AgentStreamWriter[int32]) error {
		if err := w.Write(1); err != nil {
			return err
		}
		return errors.New("disk gone")
	})
	if v, err := s.Next(); err != nil || v != 1 {
		t.Fatalf("first item %d, %v", v, err)
	}
	_, err := s.Next()
	var failure *StreamError
	if !errors.As(err, &failure) || failure.Kind != StreamFailed {
		t.Fatalf("unexpected outcome %v", err)
	}
	if value, err := DecodeTypedValue[string](failure.Value); err != nil || value != "disk gone" {
		t.Fatalf("producer error %q: %v", value, err)
	}
	if _, err := s.Next(); !errors.Is(err, io.EOF) {
		t.Fatalf("after error: %v", err)
	}

	done := ProduceStream(func(w *AgentStreamWriter[int32]) error { return w.Write(2) })
	if got, err := done.Collect(); err != nil || len(got) != 1 || got[0] != 2 {
		t.Fatalf("a finished production gave %v: %v", got, err)
	}
}

func TestAgentStreamOutcomes(t *testing.T) {
	declared := EncodeTypedValue(struct {
		Code   int32
		Reason string
	}{17, "denied"})
	cases := []struct {
		kind StreamErrorKind
		wire types.StreamError
	}{
		{StreamFailed, types.MakeStreamErrorFailed(declared.wit)},
		{StreamTrapped, types.MakeStreamErrorTrapped("trap")},
		{StreamCancelled, types.MakeStreamErrorCancelled()},
		{StreamAbandoned, types.MakeStreamErrorAbandoned()},
		{StreamInterrupted, types.MakeStreamErrorInterrupted("lost hop")},
		{StreamResourceExhausted, types.MakeStreamErrorResourceExhausted("limit")},
	}
	for _, tc := range cases {
		t.Run(fmt.Sprint(tc.kind), func(t *testing.T) {
			s, _ := streamOverFake(t, int32(17), int32(93))
			s.st.src.outcome = func() error { return streamErrorFromWire(tc.wire) }
			for _, expected := range []int32{17, 93} {
				if got, err := s.Next(); err != nil || got != expected {
					t.Fatalf("prefix: %v %v", got, err)
				}
			}
			_, err := s.Next()
			var failure *StreamError
			if !errors.As(err, &failure) || failure.Kind != tc.kind {
				t.Fatalf("outcome: %v", err)
			}
			if errors.Is(err, context.Canceled) != (tc.kind == StreamCancelled) {
				t.Fatalf("cancellation: %v", err)
			}
			if tc.kind == StreamFailed && !reflect.DeepEqual(failure.Value, declared) {
				t.Fatalf("declared value lost: %v", failure.Value)
			}
			if _, err := s.Next(); !errors.Is(err, io.EOF) {
				t.Fatalf("after failure: %v", err)
			}
		})
	}
}

func TestAgentStreamIteratorCarriesDeclaredFailure(t *testing.T) {
	type failureValue struct {
		Code   int32
		Reason string
	}
	declared := failureValue{29, "rejected"}
	s := ProduceStream(func(w *AgentStreamWriter[int32]) error {
		if err := w.WriteAll(17, 93); err != nil {
			return err
		}
		return &StreamError{Kind: StreamFailed, Value: EncodeTypedValue(declared)}
	})
	var prefix []int32
	failures := 0
	for value, err := range s.All() {
		if err == nil {
			prefix = append(prefix, value)
			continue
		}
		failures++
		var failure *StreamError
		if !errors.As(err, &failure) {
			t.Fatal(err)
		}
		if got, err := DecodeTypedValue[failureValue](failure.Value); err != nil || got != declared {
			t.Fatalf("value: %v %v", got, err)
		}
	}
	if !reflect.DeepEqual(prefix, []int32{17, 93}) || failures != 1 {
		t.Fatalf("prefix %v, failures %d", prefix, failures)
	}
}

// streamTree reaches its stream only through itself: the case an approximate
// propagation could miss.
type streamTree struct {
	Children []streamTree
	Leaf     Option[streamLeaf]
}

type streamLeaf struct{ Lines AgentStream[string] }

// A producer that returns the Write error after the consumer stopped reading
// ends cleanly: the reader going away is not a failed production.
func TestAProducerStoppedByTheReaderEndsCleanly(t *testing.T) {
	stopped := make(chan error, 1)
	s := ProduceStream(func(w *AgentStreamWriter[int32]) error {
		for i := int32(0); ; i++ {
			if err := w.Write(i); err != nil {
				stopped <- err
				return err
			}
		}
	})
	if _, err := s.Next(); err != nil {
		t.Fatal("no first item")
	}
	s.Close()
	if err := <-stopped; !errors.Is(err, ErrReaderGone) {
		t.Fatalf("the producer stopped with %v, want ErrReaderGone", err)
	}
}
