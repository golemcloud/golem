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

//go:build wasip1

package golem

import (
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// These three constructors are the only places a generated stream resource is
// bound to the stream interfaces. Binding one materializes its Drop method,
// which is a host call, so doing it in a shared file would drag
// //go:wasmimport symbols into a native link that has no bodies for them.

func newStreamPair() (treeSink, treeSource) {
	w, r := types.MakeStreamSchemaValueTree()
	return sinkOf(w), sourceOf(r)
}

// sourceOf and sinkOf bind a host endpoint to the function-struct the stream
// logic uses. They are the only places a generated stream endpoint is touched.
func sourceOf(r *witTypes.StreamReader[types.SchemaValueTree]) treeSource {
	return treeSource{
		read:          r.Read,
		writerDropped: r.WriterDropped,
		drop:          r.Drop,
		reader:        r,
	}
}

func sinkOf(w *witTypes.StreamWriter[types.SchemaValueTree]) treeSink {
	return treeSink{
		write:         w.Write,
		readerDropped: w.ReaderDropped,
		drop:          w.Drop,
	}
}

// streamSourceFromNode takes the reading endpoint out of a received stream
// value node.
func streamSourceFromNode(n types.SchemaValueNode) (treeSource, bool) {
	return sourceOf(types.SchemaValueStreamUnwrap(n.StreamValue())), true
}

// streamNodeFrom turns a reading endpoint back into a value node for transfer.
// Only an endpoint that came from the host can go back to it.
func streamNodeFrom(src treeSource) (types.SchemaValueNode, bool) {
	r, ok := src.reader.(*witTypes.StreamReader[types.SchemaValueTree])
	if !ok {
		return types.SchemaValueNode{}, false
	}
	return types.MakeSchemaValueNodeStreamValue(types.SchemaValueStreamWrap(r)), true
}

// streamTypeBody builds the stream type node for an item type.
func streamTypeBody(item int32) types.SchemaTypeBody {
	return types.MakeSchemaTypeBodyStreamType(witTypes.Some(item))
}
