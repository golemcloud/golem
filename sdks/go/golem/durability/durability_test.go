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

package durability

import (
	"testing"

	apiOplog "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
)

// TestFunctionTypeMapping — each FunctionType maps to the expected
// host WrappedFunctionType tag. This is the pure part of the custom-durability
// surface; Run itself needs the host and is covered by the executor tests.
func TestFunctionTypeMapping(t *testing.T) {
	cases := []struct {
		name string
		ft   FunctionType
		tag  uint8
	}{
		{"ReadLocal", ReadLocal, apiOplog.WrappedFunctionTypeReadLocal},
		{"WriteLocal", WriteLocal, apiOplog.WrappedFunctionTypeWriteLocal},
		{"ReadRemote", ReadRemote, apiOplog.WrappedFunctionTypeReadRemote},
		{"WriteRemote", WriteRemote, apiOplog.WrappedFunctionTypeWriteRemote},
	}
	for _, c := range cases {
		if got := c.ft.raw.Tag(); got != c.tag {
			t.Fatalf("%s tag = %d, want %d", c.name, got, c.tag)
		}
	}
}
