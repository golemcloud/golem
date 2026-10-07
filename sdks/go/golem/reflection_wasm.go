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
	toolHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_host"
)

// DiscoverTools returns a snapshot of every tool the calling agent may reach in
// its environment.
func DiscoverTools() []ReflectedTool {
	registered := toolHost.GetAllTools()
	out := make([]ReflectedTool, 0, len(registered))
	for _, r := range registered {
		out = append(out, newReflectedTool(r.LookupName, r.Definition))
	}
	return out
}

// DiscoverTool looks one tool up by its lookup name.
func DiscoverTool(name string) (ReflectedTool, bool) {
	found := toolHost.GetTool(name)
	if found.IsNone() {
		return ReflectedTool{}, false
	}
	return newReflectedTool(found.Some().LookupName, found.Some().Definition), true
}
