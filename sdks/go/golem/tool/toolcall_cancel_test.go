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
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The host future is released by wait, so a cancel afterwards (a deferred
// Cancel, or a relay failing after the result) must not reach it.
func TestCancelAfterWaitDoesNotReachTheReleasedFuture(t *testing.T) {
	released := false
	cancels := 0
	call := guardCancel(toolCall{
		wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			released = true
			return witTypes.None[types.TypedSchemaValue](), nil
		},
		cancel: func() {
			if released {
				t.Fatal("cancel reached a released future")
			}
			cancels++
		},
	})
	call.cancel()
	if cancels != 1 {
		t.Fatalf("cancel before wait reached the future %d times, want 1", cancels)
	}
	call.wait()
	call.cancel()
	call.cancel()
}
