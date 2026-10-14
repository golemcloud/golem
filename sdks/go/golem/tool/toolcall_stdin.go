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
	"errors"
	"io"
	"slices"
	"sync"
)

// stdinSink is the caller's end of a call's standard input.
type stdinSink interface {
	// write reports false once the stream takes no more bytes.
	write(p []byte) bool
	finish()
	fail(f OutputFailure)
	release()
}

// stdinPump copies a caller's reader into a call's standard input. The tool
// may stop reading before the source ends; the watcher reports that, and the
// pump then releases the sink and closes a source that is an io.Closer, which
// is the only way to unblock a Read in Go.
type stdinPump struct {
	src  io.Reader
	sink stdinSink

	mu      sync.Mutex
	stopped bool
}

// pumpStdin runs the pump and its watcher. wait returns when the stream
// closes, for any reason, including the pump's own finish; release frees the
// watcher.
func pumpStdin(src io.Reader, sink stdinSink, wait func(), release func()) {
	p := &stdinPump{src: src, sink: sink}
	go func() {
		wait()
		release()
		if p.stop() {
			if closer, ok := src.(io.Closer); ok {
				_ = closer.Close()
			}
		}
	}()
	p.run()
}

// stop releases the sink once; it reports whether this call did it.
func (p *stdinPump) stop() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.stopped {
		return false
	}
	p.stopped = true
	p.sink.release()
	return true
}

// with runs op on the sink unless the pump has stopped; op runs under the
// lock, so the watcher cannot release the sink during it.
func (p *stdinPump) with(op func() bool) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.stopped {
		return false
	}
	return op()
}

// run selects the stream's terminal: finished at io.EOF, failed on a read
// error.
func (p *stdinPump) run() {
	defer p.stop()
	buf := make([]byte, 64*1024)
	for {
		n, err := p.src.Read(buf)
		if n > 0 && !p.with(func() bool { return p.sink.write(slices.Clone(buf[:n])) }) {
			return
		}
		switch {
		case errors.Is(err, io.EOF):
			p.with(func() bool { p.sink.finish(); return true })
			return
		case err != nil:
			failure := OutputFailed(err.Error())
			var se *OutputError
			if errors.As(err, &se) {
				failure = se.Failure
			}
			p.with(func() bool { p.sink.fail(failure); return true })
			return
		}
	}
}
