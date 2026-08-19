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
	"errors"
	"fmt"
	"os"
	"reflect"

	guestExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_agent_guest"
	loadExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_api_load_snapshot"
	saveExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_api_save_snapshot"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	apihost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
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
// rather than from the agent's handler — i.e. whether it is our bug. An
// [encodeError] is the agent supplying an unencodable value, so it is not
// internal even though it surfaces in the encode stage.
func (e *PanicError) Internal() bool {
	if _, ok := e.Value.(*encodeError); ok {
		return false
	}
	return e.Stage != stageHandler
}

func (e *PanicError) Error() string {
	if ee, ok := e.Value.(*encodeError); ok {
		return fmt.Sprintf("agent method %q returned a value that cannot be encoded: %s", e.Method, ee.Error())
	}
	if e.Internal() {
		return fmt.Sprintf("INTERNAL SDK ERROR while %s for method %q: %v", e.Stage, e.Method, e.Value)
	}
	return fmt.Sprintf("agent method %q panicked: %v", e.Method, e.Value)
}

// Unwrap exposes the recovered value when it is itself an error, so a wrapped
// cause (e.g. a RemoteCallError from a nested call, or an encodeError) stays
// reachable via errors.As/Is.
func (e *PanicError) Unwrap() error {
	if err, ok := e.Value.(error); ok {
		return err
	}
	return nil
}

// encodeError marks an encode-stage panic caused by the agent supplying a value
// the wire cannot carry — a nil or unregistered variant case, an out-of-range
// enum value, or a Secret used as a parameter/return. It is the agent's mistake,
// so it is reported as an agent error rather than an INTERNAL SDK error.
type encodeError struct{ msg string }

func (e *encodeError) Error() string { return e.msg }

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
		// Route structured logging (slog, and via it the standard log package)
		// through the host logging channel so it carries a real level + context.
		// Build-tag-gated to the wasm target so native `go test` never links the
		// host call (see loginstall_*.go).
		installDefaultLogger()
		if _, ds := defs.discover(); agentDefErrors(ds, agentType) != "" {
			return witTypes.Err[witTypes.Unit](customError("agent definition errors:\n" + agentDefErrors(ds, agentType)))
		}
		e := defs.agents[agentType]
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
		// Publish the instance before running the constructor so a constructor that
		// reads config (via ctx.Config()) populates the same per-worker config cache
		// the methods use. The constructor sees only its InitContext, never the
		// not-yet-built state, so the nil state during this window is unobservable.
		agentID := os.Getenv(agentIDEnvVar)
		inst := &instance{def: e, agentID: agentID}
		active = inst
		inst.state = e.newState(idVal, agentID)
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

	// GetDefinition has no error channel in the WIT. It is only valid after a
	// successful initialize, and initialize refuses an agent with definition
	// errors — so by the time this runs the type is already validated.
	guestExports.Exports.GetDefinition = func() common.AgentType {
		types, _ := defs.discover()
		want := ""
		if active != nil {
			want = active.def.name
		} else if len(defs.order) > 0 {
			want = defs.order[0]
		}
		for _, at := range types {
			if at.TypeName == want {
				return at
			}
		}
		return common.AgentType{}
	}

	guestExports.Exports.DiscoverAgentTypes = func() witTypes.Result[[]common.AgentType, common.AgentError] {
		types, ds := defs.discover()
		if len(ds) > 0 {
			return witTypes.Err[[]common.AgentType](customError(allDefErrors(ds)))
		}
		return witTypes.Ok[[]common.AgentType, common.AgentError](types)
	}

	// save/load-snapshot serialize and restore the running instance's state. save
	// has no error channel in the WIT, so a failure there is fatal (panic); load
	// reports failures through its result.
	saveExports.Exports.Save = func() apihost.Snapshot {
		if active == nil {
			panic(fmt.Errorf("golem: save-snapshot before initialize"))
		}
		snap, err := saveState(active.state)
		if err != nil {
			panic(fmt.Errorf("golem: snapshot save failed: %w", err))
		}
		return snap
	}

	loadExports.Exports.Load = func(snap apihost.Snapshot) witTypes.Result[witTypes.Unit, string] {
		if active == nil {
			return witTypes.Err[witTypes.Unit]("agent not initialized")
		}
		if err := loadState(active.state, snap); err != nil {
			return witTypes.Err[witTypes.Unit](err.Error())
		}
		return witTypes.Ok[witTypes.Unit, string](witTypes.Unit{})
	}
}
