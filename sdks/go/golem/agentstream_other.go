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
	"errors"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Off the wasm target there is no host to stream through. The stream logic
// itself is target-independent and tested directly against fakes; only these
// three constructors are not, because binding a generated stream resource to an
// interface pulls its //go:wasmimport methods into a native link.

var errNoHostStreams = errors.New("golem: agent streams are only available inside a component")

func newStreamPair() (treeSink, treeSource) {
	return treeSink{
		write:         func([]types.SchemaValueTree) uint32 { return 0 },
		readerDropped: func() bool { return true },
		drop:          func() {},
	}, treeSource{
		read:          func([]types.SchemaValueTree) uint32 { return 0 },
		writerDropped: func() bool { return true },
		drop:          func() {},
	}
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
