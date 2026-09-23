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
	toolExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_tool_guest"
)

// newToolStdin adapts the host-supplied reader, or produces one that explains
// its own absence.
func newToolStdin(stdin toolExports.Stdin) *ToolStdin {
	if stdin.IsNone() {
		return &ToolStdin{absent: absentStdin}
	}
	return &ToolStdin{src: stdin.Some()}
}

// newToolStdout adapts the host-supplied writer, or produces one that explains
// its own absence.
func newToolStdout(stdout toolExports.Stdout) *ToolStdout {
	if stdout.IsNone() {
		return &ToolStdout{absent: absentStdout}
	}
	return &ToolStdout{sink: stdout.Some()}
}
