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

package golem

import (
	"fmt"
	"io"
	"time"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
)

// Fully dynamic clients.
//
// These are for callers that already hold schema-native values — an
// infrastructure transport, or a caller that packed them with a discovered
// snapshot — and keep no schema authority. Nothing is validated locally: the
// host still authorises the call and validates the target-side input.
//
//	client, err := golem.BindAgentID(id)
//	out, invocation, err := client.Call("greet", input)
//
// To combine discovery with a dynamic call, pack with the discovered method's
// Input, call, and read the result with its Output.

// RawAgentID is an agent identity taken apart by the host, with the
// constructor left as a typed value rather than decoded into a Go type — which
// is the point: a dynamic caller has no such type. The typed counterpart is
// [ParseAgentID].
type RawAgentID struct {
	// TypeName is the agent type the identity names.
	TypeName string
	// Constructor carries the caller-supplied constructor fields. The host
	// injects principal fields separately, so a principal-scoped agent can be
	// rebuilt from an identity returned by invocation metadata.
	Constructor TypedValue
	// PhantomID is the phantom instance this identity addresses, if any.
	PhantomID Option[UUID]
}

// ConstructorJSON reads the constructor fields as canonical JSON.
func (p RawAgentID) ConstructorJSON() (any, error) { return p.Constructor.JSON() }

// DynamicAgentClient invokes an existing agent with values the caller packed
// itself.
type DynamicAgentClient struct {
	agentID string
	parsed  RawAgentID
	rpc     agentRPC
}

// AgentID returns the identity this client is bound to.
func (c *DynamicAgentClient) AgentID() string { return c.agentID }

// Parsed returns what the host made of the identity.
func (c *DynamicAgentClient) Parsed() RawAgentID { return c.parsed }

// Call invokes a method with an already-packed parameter record and waits for
// its result, none for a method that returns nothing, together with the
// invocation's identity.
func (c *DynamicAgentClient) Call(method string, input core.SchemaValue) (Option[core.SchemaValue], InvocationID, error) {
	tree, err := witschema.ValueToWit(input)
	if err != nil {
		return None[core.SchemaValue](), InvocationID{}, fmt.Errorf("golem: %s: %w", method, err)
	}
	res, id, err := c.rpc.call(method, tree)
	if err != nil {
		return None[core.SchemaValue](), id, err
	}
	result, has := optionFromWit(res).Get()
	if !has {
		return None[core.SchemaValue](), id, nil
	}
	value, err := witschema.ValueToCore(result)
	if err != nil {
		return None[core.SchemaValue](), id, fmt.Errorf("golem: %s returned an unreadable result: %w", method, err)
	}
	return Some(value), id, nil
}

// Trigger invokes a method without waiting for its result.
func (c *DynamicAgentClient) Trigger(method string, input core.SchemaValue) (InvocationID, error) {
	tree, err := witschema.ValueToWit(input)
	if err != nil {
		return InvocationID{}, fmt.Errorf("golem: %s: %w", method, err)
	}
	return c.rpc.trigger(method, tree)
}

// Schedule arranges for a method to be invoked at the given time.
func (c *DynamicAgentClient) Schedule(at time.Time, method string, input core.SchemaValue) (*ScheduledInvocation, error) {
	tree, err := witschema.ValueToWit(input)
	if err != nil {
		return nil, fmt.Errorf("golem: %s: %w", method, err)
	}
	return c.rpc.schedule(at, method, tree)
}

// DynamicToolClient invokes a tool by name and command path, with values the
// caller packed itself.
type DynamicToolClient struct {
	toolName string
}

// ToolName returns the tool this client is bound to.
func (c *DynamicToolClient) ToolName() string { return c.toolName }

// Call runs a command with an already-packed input and returns the raw result,
// none for a command that produces nothing.
func (c *DynamicToolClient) Call(path []string, input TypedValue) (Option[TypedValue], error) {
	inv, err := c.Start(path, input, nil, false)
	if err != nil {
		return None[TypedValue](), err
	}
	return inv.Wait()
}

// Start starts a command with an already-packed input and the given standard
// input, which may be nil; stdout asks for the command's standard output.
func (c *DynamicToolClient) Start(path []string, input TypedValue, stdin io.Reader, stdout bool) (*ToolInvocation[Option[TypedValue]], error) {
	call, err := startToolCall(c.toolName, path, input.wit, stdin, stdout)
	if err != nil {
		return nil, err
	}
	name := c.toolName
	return &ToolInvocation[Option[TypedValue]]{call: call, finish: func(call toolCall) (Option[TypedValue], error) {
		res, rpcErr := call.wait()
		if rpcErr != nil {
			return None[TypedValue](), toolCallErrorFromWit(name, path, *rpcErr)
		}
		value, has := optionFromWit(res).Get()
		if !has {
			return None[TypedValue](), nil
		}
		return Some(TypedValue{wit: value}), nil
	}}, nil
}

// Bind connects to the discovered tool. Nothing is checked until a call: the
// tool is looked up by name when a command is invoked.
func (r ReflectedTool) Bind() (*ReflectedToolClient, error) {
	return &ReflectedToolClient{tool: r}, nil
}

// BindTool binds a tool by name without retaining its metadata.
func BindTool(toolName string) (*DynamicToolClient, error) {
	if toolName == "" {
		return nil, fmt.Errorf("golem: BindTool requires a tool name")
	}
	return &DynamicToolClient{toolName: toolName}, nil
}
