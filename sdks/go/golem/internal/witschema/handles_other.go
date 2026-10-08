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

package witschema

import (
	"fmt"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Off the wasm target there is no platform lending handles out, and binding a
// generated resource to an interface here would pull its host-call methods
// into a native link. The rest of the conversion is target-independent and
// tested directly.

var errNoHandles = fmt.Errorf("golem: host-managed handles are only available inside a component")

func handleToCore(types.SchemaValueNode) (core.SchemaValue, error) { return nil, errNoHandles }

func handleToWit(core.SchemaValue) (types.SchemaValueNode, error) {
	return types.SchemaValueNode{}, errNoHandles
}
