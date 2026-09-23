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
		t.Fatalf("Collect: %v", err)
	}
	if !reflect.DeepEqual(got, []string{"a", "b", "c"}) {
		t.Errorf("read %v", got)
	}
}

// TestAgentStreamEOFIsNotAnError — the contract a consumer relies on: a clean
// end of input is ok=false with a nil error, never an error value.
func TestAgentStreamEOFIsNotAnError(t *testing.T) {
	s, _ := streamOverFake(t, "only")
	if _, ok, err := s.Next(); !ok || err != nil {
		t.Fatalf("first read: ok=%v err=%v", ok, err)
	}
	v, ok, err := s.Next()
	if ok || err != nil {
		t.Errorf("end of input reported as ok=%v err=%v value=%v", ok, err, v)
	}
}

func TestAgentStreamAllStopsAtEOF(t *testing.T) {
	s, _ := streamOverFake(t, int64(1), int64(2))
	var seen []int64
	for v, err := range s.All() {
		if err != nil {
			t.Fatalf("All yielded %v", err)
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
	if err := w.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if !f.dropped {
		t.Error("closing the writer did not complete the stream")
	}
	// Closing twice is not an error; a deferred Close after an explicit one is
	// the normal Go shape.
	if err := w.Close(); err != nil {
		t.Errorf("second Close gave %v", err)
	}
	if err := w.Write("b"); !errors.Is(err, ErrStreamClosed) {
		t.Errorf("write after close gave %v", err)
	}
}

// TestAgentStreamTransferIsAffine — handing a stream on moves the endpoint;
// aliases share the state, so a second transfer through any copy is refused.
func TestAgentStreamTransferIsAffine(t *testing.T) {
	s, _ := streamOverFake(t, "a")
	alias := s

	if _, err := s.streamTake(); err != nil {
		t.Fatalf("first transfer: %v", err)
	}
	if _, err := alias.streamTake(); !errors.Is(err, ErrStreamTransferred) {
		t.Errorf("second transfer through an alias gave %v, want ErrStreamTransferred", err)
	}
	if _, _, err := s.Next(); !errors.Is(err, ErrStreamTransferred) {
		t.Errorf("reading a transferred stream gave %v", err)
	}
}

// TestAgentStreamPartiallyReadCannotBeForwarded — the items already taken
// cannot be put back, so the receiver would not get the stream it was promised.
func TestAgentStreamPartiallyReadCannotBeForwarded(t *testing.T) {
	s, _ := streamOverFake(t, "a", "b")
	if _, ok, err := s.Next(); !ok || err != nil {
		t.Fatalf("first read: ok=%v err=%v", ok, err)
	}
	if _, err := s.streamTake(); !errors.Is(err, ErrStreamPartiallyRead) {
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

// TestAgentStreamDecodeErrorIsNotEOF — a malformed item must be distinguishable
// from the end of the stream, or a consumer silently truncates.
func TestAgentStreamDecodeErrorIsNotEOF(t *testing.T) {
	var b valBuilder
	root := b.push(types.MakeSchemaValueNodeStringValue("not an int"))
	f := &fakeStream{
		dropped: true,
		items:   []types.SchemaValueTree{{ValueNodes: b.nodes, Root: root}},
	}
	s := AgentStream[int64]{st: &streamState{src: f.source()}, codec: defaultStreamCodec[int64]()}

	_, ok, err := s.Next()
	if ok {
		t.Fatal("a malformed item was accepted")
	}
	if err == nil {
		t.Fatal("a malformed item was reported as the end of the stream")
	}
}

// TestStreamSchemaIsAlwaysTyped — an untyped stream cannot supply an item
// codec, and Go has no way to spell one.
func TestStreamSchemaIsAlwaysTyped(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[AgentStream[string]]()))
	body := g.build().TypeNodes[root].Body
	if body.Tag() != types.SchemaTypeBodyStreamType {
		t.Fatalf("AgentStream lowered to tag %d, want stream-type", body.Tag())
	}
	item := body.StreamType()
	if item.IsNone() {
		t.Fatal("the stream carries no item type")
	}
	if tag := g.build().TypeNodes[item.Some()].Body.Tag(); tag != types.SchemaTypeBodyStringType {
		t.Errorf("item type tag %d, want string", tag)
	}
}

// TestContainsStreamPropagatesThroughComposites — the flag has to survive
// nesting, since a stream buried in a record still rules out Trigger.
func TestContainsStreamPropagatesThroughComposites(t *testing.T) {
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
	} {
		d := newDefinitions()
		if got := d.compile(tc.rt).containsStream; got != tc.want {
			t.Errorf("%s: containsStream=%v, want %v", tc.name, got, tc.want)
		}
	}
}
