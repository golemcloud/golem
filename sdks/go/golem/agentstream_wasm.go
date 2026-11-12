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
	fw, fr := types.MakeFutureOptionResultUnitStreamError()
	sink, src := sinkOf(w), sourceOf(r)
	sink.drop = func() { w.Drop(); fw.Drop() }
	sink.finish = func(err error) {
		w.Drop()
		outcome := witTypes.Ok[witTypes.Unit, types.StreamError](witTypes.Unit{})
		if err != nil {
			outcome = witTypes.Err[witTypes.Unit, types.StreamError](streamFailure(err))
		}
		fw.Write(witTypes.Some(outcome))
	}
	src.future = fr
	src.outcome = func() error {
		outcome := fr.Read()
		if outcome.IsNone() {
			return &StreamError{Kind: StreamAbandoned}
		}
		if outcome.Some().IsErr() {
			return streamErrorFromWire(outcome.Some().Err())
		}
		return nil
	}
	src.drop = func() { r.Drop(); fr.Drop() }
	return sink, src
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
	r, outcome := types.SchemaValueStreamUnwrap(n.StreamValue())
	src := sourceOf(r)
	src.outcome = func() error {
		result := outcome.Read()
		if result.IsErr() {
			return streamErrorFromWire(result.Err())
		}
		return nil
	}
	src.dropOutcome = outcome.Drop
	src.drop = func() { r.Drop(); outcome.Drop() }
	return src, true
}

// streamNodeFrom turns a reading endpoint back into a value node for transfer.
// Only an endpoint that came from the host can go back to it.
func streamNodeFrom(src treeSource) (types.SchemaValueNode, bool) {
	r, ok := src.reader.(*witTypes.StreamReader[types.SchemaValueTree])
	if !ok {
		return types.SchemaValueNode{}, false
	}
	outcome, ok := src.future.(*witTypes.FutureReader[witTypes.Option[types.StreamOutcome]])
	if !ok {
		// The host recognizes an unread forwarded input and retains its outcome.
		writer, reader := types.MakeFutureOptionResultUnitStreamError()
		writer.Drop()
		outcome = reader
	}
	wrapped := types.SchemaValueStreamWrap(r, outcome)
	if src.dropOutcome != nil {
		src.dropOutcome()
	}
	return types.MakeSchemaValueNodeStreamValue(wrapped), true
}

// streamTypeBody builds the stream type node for an item type.
func streamTypeBody(item int32) types.SchemaTypeBody {
	return types.MakeSchemaTypeBodyStreamType(witTypes.Some(item))
}
