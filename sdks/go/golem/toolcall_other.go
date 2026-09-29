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
	"fmt"
	"io"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Off the wasm target there is no host to call through; see toolcall_wasm.go.
func startToolCallHost(tool string, path []string, _ types.TypedSchemaValue, _ io.Reader, _ bool) (toolCall, error) {
	return toolCall{}, fmt.Errorf("golem: calling tool %s %s is only available inside a component", tool, commandLabel(path))
}
