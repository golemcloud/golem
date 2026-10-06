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

package durablestreams

import wire "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_durable_streams"

// Off the wasm target there is no host to read or write through; see
// durablestreams_wasm.go. Tests replace the calls on the Reader or Writer.

func outsideComponent() *Error {
	return newError(KindUnavailable, "durable streams are only available inside a component")
}

func hostReader(string, wire.DurableStreamMode, ReadOptions) readCall {
	return func(wire.DurableStreamReadRequest) (wire.DurableStreamBatch, *Error) {
		return wire.DurableStreamBatch{}, outsideComponent()
	}
}

func hostWriter(string, string, string, WriteOptions) appendCall {
	return func(wire.DurableStreamAppendRequest) (wire.DurableStreamAppendReceipt, *Error) {
		return wire.DurableStreamAppendReceipt{}, outsideComponent()
	}
}
