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

	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
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

// TrapPanic reports a panic recovered in a region that must not resume, such
// as a durable operation, and traps the component. Off the wasm target it panics
// again with the value.
var TrapPanic func(region string, value any)

// TrapOnPanic runs f and traps the component if f panics, so that no caller
// can recover the panic and go on inside a region that must not resume.
// region names it in the report.
func TrapOnPanic(region string, f func()) {
	defer func() {
		if r := recover(); r != nil {
			TrapPanic(region, r)
		}
	}()
	f()
}

// The root package's own types, built and read from subpackages. Each returns
// or takes the root type as any: a subpackage asserts it back.
var (
	// TypedValue wraps a wire value as a golem.TypedValue.
	TypedValue func(types.TypedSchemaValue) any
	// TypedValueWit unwraps a golem.TypedValue.
	TypedValueWit func(any) types.TypedSchemaValue
	// Caller returns a function that reads the caller of the running invocation
	// as a golem.Caller on its first call (a func() golem.Caller, as any), and a
	// function that ends the call, after which a first read panics. A call that
	// returned a stream ends with true: the end reads the caller for its producer.
	Caller func() (read any, end func(returnedStream bool))
	// AgentError converts a host agent-error into a *golem.AgentError.
	AgentError func(common.AgentError) error
	// RemoteCallError converts a host rpc-error into a *golem.RemoteCallError.
	RemoteCallError func(target, method string, e host.RpcError) error
	// LocalAgentTypes derives the agent types this component defines, as
	// discovery publishes them.
	LocalAgentTypes func() ([]common.AgentType, error)
	// InvocationMetadata converts host invocation metadata into a
	// golem.InvocationMetadata.
	InvocationMetadata func(host.InvocationMetadata) any
	// AnyInvocation builds the *golem.Invocation[any] of a reflected client,
	// whose result decode reads.
	AnyInvocation func(ops InvocationOps, method string, decode func(witTypes.Option[types.SchemaValueTree]) (any, error)) any
	// ValueInvocation builds the
	// *golem.Invocation[golem.Option[schema.SchemaValue]] of a dynamic client,
	// whose result decode reads.
	ValueInvocation func(
		ops InvocationOps, method string,
		decode func(witTypes.Option[types.SchemaValueTree]) (values.Option[schema.SchemaValue], error),
	) any
	// ScopeCardHandle borrows the handle of a golem.ScopeCard attached to an
	// awaited call; nil and the zero ScopeCard are no scope card. It panics
	// with golem.ErrPermissionCardMoved on a released card.
	ScopeCardHandle func(card any) witTypes.Option[*types.PermissionCard]
	// ReleaseCard gives a permission-card handle back to the host.
	ReleaseCard func(*types.PermissionCard)
)

// InvocationOps are the operations of the handle of one accepted agent
// invocation, as the host answers them. A handle calls through these functions
// rather than the generated resource, so that it links and tests off the wasm
// target; [HostInvocationOps] builds them for a host resource.
type InvocationOps struct {
	// ID is the ID of the invocation.
	ID types.InvocationId
	// Status reads the invocation's lifecycle state.
	Status func() witTypes.Result[host.InvocationStatus, host.RpcError]
	// Await waits for the invocation's result.
	Await func() witTypes.Result[witTypes.Option[types.SchemaValueTree], host.RpcError]
	// Cancel withdraws the invocation if it has not started.
	Cancel func() witTypes.Result[bool, host.RpcError]
}

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
