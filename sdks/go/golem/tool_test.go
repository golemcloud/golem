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

	toolExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_guest"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// TestToolExportsAnswerWithNoToolsRegistered — the interface is part of the
// world, so every component exports it. A host probing a component that
// declares no tools must get an empty list and a named error, not a trap.
func TestToolExportsAnswerWithNoToolsRegistered(t *testing.T) {
	discovered := toolExports.DiscoverTools()
	if discovered.Tag() != witTypes.ResultOk {
		t.Fatalf("discover-tools failed on a component with no tools")
	}
	if got := discovered.Ok(); len(got) != 0 {
		t.Errorf("discover-tools returned %d tools, want none", len(got))
	}

	got := toolExports.GetTool("absent")
	if got.Tag() != witTypes.ResultErr {
		t.Fatalf("get-tool succeeded for an unregistered name")
	}
	if tag := got.Err().Tag(); tag != types.ToolErrorInvalidToolName {
		t.Errorf("get-tool error tag %d, want invalid-tool-name", tag)
	}

	invoked := toolExports.Invoke(
		"absent", nil, types.TypedSchemaValue{},
		toolExports.Stdin{}, toolExports.Stdout{},
		common.MakePrincipalAnonymous(),
	)
	if invoked.Tag() != witTypes.ResultErr {
		t.Fatalf("invoke succeeded for an unregistered tool")
	}
	if tag := invoked.Err().Tag(); tag != types.ToolErrorInvalidToolName {
		t.Errorf("invoke error tag %d, want invalid-tool-name", tag)
	}
}
