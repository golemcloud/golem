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
	"testing"

	apiOplog "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
)

// TestDurableFunctionTypeMapping — each DurableFunctionType maps to the expected
// host WrappedFunctionType tag. This is the pure part of the custom-durability
// surface; DurableOp itself needs the host and is covered by the executor tests.
func TestDurableFunctionTypeMapping(t *testing.T) {
	cases := []struct {
		name string
		ft   DurableFunctionType
		tag  uint8
	}{
		{"ReadLocal", ReadLocal, apiOplog.WrappedFunctionTypeReadLocal},
		{"WriteLocal", WriteLocal, apiOplog.WrappedFunctionTypeWriteLocal},
		{"ReadRemote", ReadRemote, apiOplog.WrappedFunctionTypeReadRemote},
		{"WriteRemote", WriteRemote, apiOplog.WrappedFunctionTypeWriteRemote},
		{"WriteRemoteBatched", WriteRemoteBatched(), apiOplog.WrappedFunctionTypeWriteRemoteBatched},
		{"WriteRemoteTransaction", WriteRemoteTransaction(), apiOplog.WrappedFunctionTypeWriteRemoteTransaction},
	}
	for _, c := range cases {
		if got := c.ft.raw.Tag(); got != c.tag {
			t.Fatalf("%s tag = %d, want %d", c.name, got, c.tag)
		}
	}
}

// TestWriteRemoteBatchedBeginIndex — the optional begin index is carried when
// given and absent otherwise, for both batched and transaction variants.
func TestWriteRemoteBatchedBeginIndex(t *testing.T) {
	if idx := WriteRemoteBatched().raw.WriteRemoteBatched(); idx.IsSome() {
		t.Fatalf("WriteRemoteBatched() should carry no begin index, got %d", idx.Some())
	}
	if idx := WriteRemoteBatched(42).raw.WriteRemoteBatched(); !idx.IsSome() || idx.Some() != 42 {
		t.Fatalf("WriteRemoteBatched(42) begin index = %v, want Some(42)", idx)
	}
	if idx := WriteRemoteTransaction().raw.WriteRemoteTransaction(); idx.IsSome() {
		t.Fatalf("WriteRemoteTransaction() should carry no begin index, got %d", idx.Some())
	}
	if idx := WriteRemoteTransaction(7).raw.WriteRemoteTransaction(); !idx.IsSome() || idx.Some() != 7 {
		t.Fatalf("WriteRemoteTransaction(7) begin index = %v, want Some(7)", idx)
	}
}
