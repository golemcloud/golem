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

// Package link carries what the root golem package shares with its
// subpackages at run time. The root sets it during package initialization,
// before any subpackage function can run.
package link

import (
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Engine is the root package's type system: the variants, enums, flags,
// unions and pins every package encodes values against.
var Engine *engine.Engine

// The root package's own types, built and read from subpackages. Each returns
// or takes the root type as any: a subpackage asserts it back.
var (
	// TypedValue wraps a wire value as a golem.TypedValue.
	TypedValue func(types.TypedSchemaValue) any
	// TypedValueWit unwraps a golem.TypedValue.
	TypedValueWit func(any) types.TypedSchemaValue
	// AgentError converts a host agent-error into a *golem.AgentError.
	AgentError func(common.AgentError) error
	// RemoteCallError converts a host rpc-error into a *golem.RemoteCallError.
	RemoteCallError func(target, method string, e host.RpcError) error
	// LocalAgentTypes derives the agent types this component defines, as
	// discovery publishes them.
	LocalAgentTypes func() ([]common.AgentType, error)
	// ScheduledInvocation builds a *golem.ScheduledInvocation.
	ScheduledInvocation func(agentID, idempotencyKey string, token *host.CancellationToken) any
)
