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
)

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
	inv, err := c.Start(path, input, nil, ToolStreams{})
	if err != nil {
		return None[TypedValue](), err
	}
	return inv.Wait()
}

// Start starts a command with an already-packed input and the given standard
// input, which may be nil, requesting the outputs streams selects. The host
// refuses a request that does not match what the command declares.
func (c *DynamicToolClient) Start(path []string, input TypedValue, stdin io.Reader, streams ToolStreams) (*ToolInvocation[Option[TypedValue]], error) {
	call, err := startToolCall(c.toolName, path, input.wit, stdin, streams)
	if err != nil {
		return nil, err
	}
	name := c.toolName
	return newInvocation(call, func(call toolCall) (Option[TypedValue], error) {
		res, rpcErr := call.wait()
		if rpcErr != nil {
			return None[TypedValue](), toolCallErrorFromWit(name, path, *rpcErr)
		}
		value, has := optionFromWit(res).Get()
		if !has {
			return None[TypedValue](), nil
		}
		return Some(TypedValue{wit: value}), nil
	}), nil
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
