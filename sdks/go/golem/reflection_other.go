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

import "fmt"

// Off the wasm target there is no deployment to discover, and binding the
// generated host calls here would drag their //go:wasmimport declarations into
// a native link that has no bodies for them. The snapshot handling itself is
// target-independent and tested directly.

func DiscoverAgentTypes() []ReflectedAgentType { return nil }

func DiscoverAgentType(string) (ReflectedAgentType, bool) { return ReflectedAgentType{}, false }

func DiscoverAgentTypeByID(string) (ReflectedAgentType, bool) { return ReflectedAgentType{}, false }

func (r ReflectedAgentType) Bind(map[string]any, ...ClientOpt) (*ReflectedAgentClient, error) {
	return nil, fmt.Errorf("golem: agent discovery is only available inside a component")
}

func (r ReflectedAgentType) BindPhantom(map[string]any, UUID, ...ClientOpt) (*ReflectedAgentClient, error) {
	return nil, fmt.Errorf("golem: agent discovery is only available inside a component")
}

func DiscoverTools() []ReflectedTool { return nil }

func DiscoverTool(string) (ReflectedTool, bool) { return ReflectedTool{}, false }

func (r ReflectedTool) Bind() (*ReflectedToolClient, error) {
	return nil, fmt.Errorf("golem: tool discovery is only available inside a component")
}

func ParseRawAgentID(string) (RawAgentID, error) {
	return RawAgentID{}, fmt.Errorf("golem: parsing an agent id is only available inside a component")
}

func BindAgentID(string) (*DynamicAgentClient, error) {
	return nil, fmt.Errorf("golem: binding an agent id is only available inside a component")
}

func BindTool(string) (*DynamicToolClient, error) {
	return nil, fmt.Errorf("golem: binding a tool is only available inside a component")
}
