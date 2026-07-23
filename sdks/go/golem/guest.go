// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"errors"
	"fmt"
	"os"
	"reflect"

	guestExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_agent_guest"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// agentIDEnvVar is how the executor tells the guest which agent instance it is.
const agentIDEnvVar = "GOLEM_AGENT_ID"

// Stages a dispatch can panic in, so a recovered panic can be attributed to the
// agent's code or to the SDK itself.
const (
	stageDecode  = "decoding input"
	stageHandler = "agent method"
	stageEncode  = "encoding output"
)

// PanicError is a recovered panic, attributed to the stage it occurred in.
type PanicError struct {
	Method string
	Stage  string
	Value  any
}

// Internal reports whether the panic came from SDK machinery (marshaling)
// rather than from the agent's handler — i.e. whether it is our bug.
func (e *PanicError) Internal() bool { return e.Stage != stageHandler }

func (e *PanicError) Error() string {
	if e.Internal() {
		return fmt.Sprintf("INTERNAL SDK ERROR while %s for method %q: %v", e.Stage, e.Method, e.Value)
	}
	return fmt.Sprintf("agent method %q panicked: %v", e.Method, e.Value)
}

// decodeError marks failures that really are the caller's bad input, so they can
// be reported as invalid-input rather than a generic failure.
type decodeError struct{ msg string }

func (e *decodeError) Error() string { return e.msg }

// customError builds agent-error's custom-error case carrying a string payload:
// the right bucket for failures that are neither bad input nor an unknown
// method (a panic is not "invalid input").
func customError(msg string) common.AgentError {
	return common.MakeAgentErrorCustomError(types.TypedSchemaValue{
		Graph: types.SchemaGraph{
			TypeNodes: []types.SchemaTypeNode{{Body: types.MakeSchemaTypeBodyStringType()}},
			Root:      0,
		},
		Value: types.SchemaValueTree{
			ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeStringValue(msg)},
			Root:       0,
		},
	})
}

// toAgentError maps an SDK or handler error onto the right agent-error case.
func toAgentError(err error) common.AgentError {
	var de *decodeError
	if errors.As(err, &de) {
		return common.MakeAgentErrorInvalidInput(de.Error())
	}
	// Panics and errors returned deliberately by agent code are both
	// "something went wrong", not malformed input.
	return customError(err.Error())
}

func init() {
	guestExports.Exports.Initialize = func(agentType string, input types.SchemaValueTree, _ common.Principal) witTypes.Result[witTypes.Unit, common.AgentError] {
		e := registry[agentType]
		if e == nil {
			return witTypes.Err[witTypes.Unit](common.MakeAgentErrorInvalidType("unknown agent type: " + agentType))
		}
		if active != nil {
			return witTypes.Err[witTypes.Unit](customError("agent already initialized"))
		}
		idVal := reflect.New(e.idType).Elem()
		if err := decodeParams(input, e.idFields, idVal); err != nil {
			return witTypes.Err[witTypes.Unit](common.MakeAgentErrorInvalidInput(err.Error()))
		}
		active = &instance{def: e, agentID: os.Getenv(agentIDEnvVar), state: e.newState(idVal)}
		return witTypes.Ok[witTypes.Unit, common.AgentError](witTypes.Unit{})
	}

	guestExports.Exports.Invoke = func(methodName string, input types.SchemaValueTree, _ common.Principal) witTypes.Result[witTypes.Option[types.SchemaValueTree], common.AgentError] {
		fail := func(e common.AgentError) witTypes.Result[witTypes.Option[types.SchemaValueTree], common.AgentError] {
			return witTypes.Err[witTypes.Option[types.SchemaValueTree]](e)
		}
		if active == nil {
			return fail(common.MakeAgentErrorInvalidMethod("agent not initialized"))
		}
		m := active.def.methods[methodName]
		if m == nil {
			return fail(common.MakeAgentErrorInvalidMethod("unknown method: " + methodName))
		}
		out, err := m.invoke(active.state, active.agentID, input)
		if err != nil {
			return fail(toAgentError(err))
		}
		if out == nil {
			return witTypes.Ok[witTypes.Option[types.SchemaValueTree], common.AgentError](
				witTypes.None[types.SchemaValueTree]())
		}
		return witTypes.Ok[witTypes.Option[types.SchemaValueTree], common.AgentError](
			witTypes.Some(*out))
	}

	guestExports.Exports.GetDefinition = func() common.AgentType {
		if active != nil {
			return buildAgentType(active.def)
		}
		if len(registryOrder) > 0 {
			return buildAgentType(registry[registryOrder[0]])
		}
		return common.AgentType{}
	}

	guestExports.Exports.DiscoverAgentTypes = func() witTypes.Result[[]common.AgentType, common.AgentError] {
		out := make([]common.AgentType, 0, len(registryOrder))
		for _, n := range registryOrder {
			out = append(out, buildAgentType(registry[n]))
		}
		return witTypes.Ok[[]common.AgentType, common.AgentError](out)
	}
}
