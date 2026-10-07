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
	"io"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Engine is the root package's type system: the variants, enums, flags,
// unions and pins every package encodes values against.
var Engine *engine.Engine

// NewEngine returns a fresh type system with the SDK's own types registered.
var NewEngine func() *engine.Engine

// The root package's own types, built and read from subpackages. Each returns
// or takes the root type as any: a subpackage asserts it back.
var (
	// TypedValue wraps a wire value as a golem.TypedValue.
	TypedValue func(types.TypedSchemaValue) any
	// TypedValueWit unwraps a golem.TypedValue.
	TypedValueWit func(any) types.TypedSchemaValue
	// Principal converts a host principal into a golem.Principal.
	Principal func(common.Principal) any
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

// Tools, set by the tool package when a component uses it.
var (
	// DiscoverTools derives every registered tool and middleware, recording
	// their definition errors on [Engine].
	DiscoverTools func()
	// StartToolCall starts a call of a tool command through the host, with
	// streams a tool.Streams. It returns the running *tool.Invocation[O], whose
	// result is finish's: O is golem.Option[golem.TypedValue] when dynamic, and
	// any otherwise. A host failure is a *tool.CallError.
	StartToolCall func(
		toolName string, path []string, input types.TypedSchemaValue, stdin io.Reader, streams any, dynamic bool,
		finish func(witTypes.Option[types.TypedSchemaValue]) (any, error),
	) (any, error)
	// ToolMetadata reads a tool.Metadata: the tool's lookup name and published
	// metadata.
	ToolMetadata func(any) (string, toolCommon.Tool)
)
