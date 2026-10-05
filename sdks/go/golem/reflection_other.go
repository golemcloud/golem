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

var errOutsideComponent = fmt.Errorf("golem: agent discovery and binding are only available inside a component")

func DiscoverAgentTypes() []ReflectedAgentType { return nil }

func DiscoverAgentType(string) (ReflectedAgentType, bool) { return ReflectedAgentType{}, false }

func DiscoverAgentTypeByID(string) (ReflectedAgentType, bool) { return ReflectedAgentType{}, false }

func (r ReflectedAgentType) Get(map[string]any, ...ClientOpt) (*ReflectedAgentClient, error) {
	return nil, errOutsideComponent
}

func (r ReflectedAgentType) NewPhantom(map[string]any, ...ClientOpt) (*ReflectedAgentClient, error) {
	return nil, errOutsideComponent
}

func (r ReflectedAgentType) Bind(string, ...ClientOpt) (*ReflectedAgentClient, error) {
	return nil, errOutsideComponent
}

func (r ReflectedAgentType) AgentID(map[string]any, Option[UUID]) (string, error) {
	return "", errOutsideComponent
}

func DiscoverTools() []ReflectedTool { return nil }

func DiscoverTool(string) (ReflectedTool, bool) { return ReflectedTool{}, false }

func ParseRawAgentID(string) (RawAgentID, error) { return RawAgentID{}, errOutsideComponent }

func BindAgentID(string) (*DynamicAgentClient, error) { return nil, errOutsideComponent }

func MakeAgentID(string, TypedValue, Option[UUID]) (string, error) { return "", errOutsideComponent }
