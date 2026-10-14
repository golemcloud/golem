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

// Package tool defines command-line tools that agents and other components
// call with typed arguments, calls them, and wraps them with middleware.
package tool

import (
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// definitions is the type system tool declarations are compiled against: the
// one the golem package's agents use, so a type means the same in both.
type definitions struct{ *engine.Engine }

// defs is the process-wide type system, shared with the golem package.
var defs = &definitions{Engine: link.Engine}

// newDefinitions is a fresh type system with the SDK's own types registered,
// for tests that must not share state.
func newDefinitions() *definitions { return &definitions{Engine: link.NewEngine()} }

func typedValue(w types.TypedSchemaValue) golem.TypedValue {
	return link.TypedValue(w).(golem.TypedValue)
}

func witOf(v golem.TypedValue) types.TypedSchemaValue { return link.TypedValueWit(v) }

func principalFromWit(p common.Principal) golem.Principal { return link.Principal(p).(golem.Principal) }
