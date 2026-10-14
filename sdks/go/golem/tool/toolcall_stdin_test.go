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

package tool

import (
	"io"
	"strings"
	"sync"
	"testing"
	"time"
)

// fakeStdin records what the pump does with a call's standard input.
type fakeStdin struct {
	mu       sync.Mutex
	written  string
	finished bool
	failed   bool
	released int
	// refuse makes every write report a closed stream.
	refuse bool
}

func (f *fakeStdin) write(p []byte) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.released > 0 {
		panic("a write reached a released sink")
	}
	if f.refuse {
		return false
	}
	f.written += string(p)
	return true
}

func (f *fakeStdin) finish() {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.released > 0 {
		panic("finish reached a released sink")
	}
	f.finished = true
}

func (f *fakeStdin) fail(OutputFailure) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.released > 0 {
		panic("fail reached a released sink")
	}
	f.failed = true
}

func (f *fakeStdin) release() {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.released++
}

// stdinState is what a fakeStdin recorded.
type stdinState struct {
	written  string
	finished bool
	failed   bool
	released int
}

func (f *fakeStdin) state() stdinState {
	f.mu.Lock()
	defer f.mu.Unlock()
	return stdinState{written: f.written, finished: f.finished, failed: f.failed, released: f.released}
}

// watcher stands in for the tool-stdin-closed watcher: closeStream plays the
// stream closing.
type watcher struct {
	closed   chan struct{}
	released chan struct{}
}

func newWatcher() *watcher {
	return &watcher{closed: make(chan struct{}), released: make(chan struct{})}
}

func (w *watcher) wait()        { <-w.closed }
func (w *watcher) release()     { close(w.released) }
func (w *watcher) closeStream() { close(w.closed) }

func pumpInBackground(src io.Reader, sink stdinSink, w *watcher) chan struct{} {
	done := make(chan struct{})
	go func() {
		pumpStdin(src, sink, w.wait, w.release)
		close(done)
	}()
	return done
}

func within(t *testing.T, what string, ch <-chan struct{}) {
	t.Helper()
	select {
	case <-ch:
	case <-time.After(5 * time.Second):
		t.Fatalf("%s did not happen", what)
	}
}

// The tool returns without reading stdin while the caller's reader blocks: the
// pipe is closed, so the blocked Read returns, and the call's handles are
// released.
func TestAToolThatStopsReadingClosesABlockedSource(t *testing.T) {
	r, w := io.Pipe()
	defer func() { _ = w.Close() }()
	sink := &fakeStdin{}
	watch := newWatcher()
	done := pumpInBackground(r, sink, watch)

	watch.closeStream()
	within(t, "the pump ending", done)
	within(t, "the watcher's release", watch.released)
	if _, err := w.Write([]byte("late")); err != io.ErrClosedPipe {
		t.Fatalf("the source was not closed: %v", err)
	}
	if got := sink.state(); got.released != 1 || got.finished || got.failed {
		t.Fatalf("sink after the tool stopped: %+v", got)
	}
}

// closingReader is a source that records whether it was closed.
type closingReader struct {
	io.Reader
	closed bool
}

func (c *closingReader) Close() error {
	c.closed = true
	return nil
}

// A source the pump reads to its end is finished, and the stream closing
// afterwards does not close it.
func TestASourceReadToItsEndIsFinishedAndNotClosed(t *testing.T) {
	src := &closingReader{Reader: strings.NewReader("haystack")}
	sink := &fakeStdin{}
	watch := newWatcher()
	done := pumpInBackground(src, sink, watch)
	within(t, "the pump ending", done)

	watch.closeStream()
	within(t, "the watcher's release", watch.released)
	got := sink.state()
	if got.written != "haystack" || !got.finished || got.released != 1 {
		t.Fatalf("sink: %+v", got)
	}
	if src.closed {
		t.Fatal("a source read to its end was closed")
	}
}

// A write the stream refuses stops the pump; the sink is released once.
func TestARefusedWriteStopsThePump(t *testing.T) {
	sink := &fakeStdin{refuse: true}
	watch := newWatcher()
	done := pumpInBackground(strings.NewReader("x"), sink, watch)
	within(t, "the pump ending", done)
	watch.closeStream()
	within(t, "the watcher's release", watch.released)
	if got := sink.state(); got.released != 1 || got.finished || got.failed {
		t.Fatalf("sink: %+v", got)
	}
}
