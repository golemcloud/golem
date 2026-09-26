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

//go:build !wasip1

package golem

import (
	"sync"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Off the wasm target there is no host to stream through, so a stream pair is an
// in-memory pipe: what the writer sends, the reader receives, and a read waits
// until there is something to read or the writer is gone. That lets code which
// produces and consumes streams run in native unit tests. Only the constructors
// here are target-specific, because binding a generated stream resource to an
// interface pulls its //go:wasmimport methods into a native link.

func newStreamPair() (treeSink, treeSource) {
	p := &memoryPipe{}
	p.ready = sync.NewCond(&p.mu)
	return treeSink{
		write:         p.write,
		readerDropped: p.isReaderGone,
		drop:          p.dropWriter,
	}, treeSource{
		read:          p.read,
		writerDropped: p.isWriterGone,
		drop:          p.dropReader,
	}
}

// memoryPipe is the native stand-in for a host stream. Like the host's, it has
// a single end state: the writer dropping it is end of input.
type memoryPipe struct {
	mu         sync.Mutex
	ready      *sync.Cond
	items      []types.SchemaValueTree
	writerGone bool
	readerGone bool
}

func (p *memoryPipe) write(items []types.SchemaValueTree) uint32 {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.readerGone {
		return 0
	}
	p.items = append(p.items, items...)
	p.ready.Broadcast()
	return uint32(len(items))
}

func (p *memoryPipe) read(dst []types.SchemaValueTree) uint32 {
	p.mu.Lock()
	defer p.mu.Unlock()
	for len(p.items) == 0 && !p.writerGone && !p.readerGone {
		p.ready.Wait()
	}
	n := copy(dst, p.items)
	p.items = p.items[n:]
	return uint32(n)
}

func (p *memoryPipe) isWriterGone() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.writerGone && len(p.items) == 0
}

func (p *memoryPipe) isReaderGone() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.readerGone
}

func (p *memoryPipe) dropWriter() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.writerGone = true
	p.ready.Broadcast()
}

func (p *memoryPipe) dropReader() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.readerGone = true
	p.items = nil
	p.ready.Broadcast()
}

func streamSourceFromNode(types.SchemaValueNode) (treeSource, bool) {
	return treeSource{}, false
}

func streamNodeFrom(treeSource) (types.SchemaValueNode, bool) {
	return types.SchemaValueNode{}, false
}

// streamTypeBody is target-independent in effect, but lives here too so the
// schema shape can be derived and tested natively without the value-node path.
func streamTypeBody(item int32) types.SchemaTypeBody {
	return types.MakeSchemaTypeBodyStreamType(witTypes.Some(item))
}
