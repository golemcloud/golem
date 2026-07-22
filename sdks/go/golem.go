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

// Package golem is the Golem Go SDK for authoring agents.
//
// An agent is declared with [DefineAgent], its methods with [DefineMethod], and
// their behaviour bound with [Implement]:
//
//	type CounterId struct{ Name string }     // constructor params; also the type-level marker
//	type CounterState struct{ count int64 }  // private state
//	type AddIn struct{ By int64 }
//
//	var Counter = golem.DefineAgent[CounterId, CounterState](
//	    golem.Spec{Name: "CounterAgent", Mode: golem.Durable},
//	    func(id CounterId) *CounterState { return &CounterState{} },
//	)
//
//	var Add = golem.DefineMethod[CounterId, AddIn, int64]("add")
//
//	func init() {
//	    golem.Implement(Counter, Add, func(ctx *golem.Context[CounterState], in AddIn) (int64, error) {
//	        ctx.State.count += in.By
//	        return ctx.State.count, nil
//	    })
//	}
//
//	func main() {} // the SDK wires the component exports from its own init()
//
// Method descriptors are package-level values so the same value drives the
// agent-type schema, the implementation binding, and (typed) calls from other
// agents. Schemas are derived from the Go types by reflection; there is no code
// generation step.
//
// Importing this package links the generated golem:agent/guest export glue into
// the component, so an agent's main package needs only:
//
//	import _ "github.com/golemcloud/golem-go"
package golem

import (
	common "github.com/golemcloud/golem-go/internal/wit/golem_agent_common"

	// Links the generated wasmexport glue into the component.
	_ "github.com/golemcloud/golem-go/internal/wit/wit_exports"
)

// Unit is the empty input/output marker. Go has no arity overloading, so a
// method with no parameters takes Unit, and one with no result returns Unit.
type Unit struct{}

// Mode selects an agent's durability model.
type Mode uint8

const (
	// Durable agents persist their state and replay deterministically.
	Durable Mode = iota
	// Ephemeral agents do not persist state across invocations.
	Ephemeral
)

func (m Mode) toWit() common.AgentMode {
	if m == Ephemeral {
		return common.AgentModeEphemeral
	}
	return common.AgentModeDurable
}

// Spec is the declarative part of an agent definition.
type Spec struct {
	// Name is the wire-level agent type name (as seen by the platform and by
	// other agents). Required.
	Name string
	// Description is surfaced in the agent type metadata.
	Description string
	// Mode defaults to Durable.
	Mode Mode
}

// Context is passed to every method handler. State is the agent instance's
// private state, as returned by the agent's init function.
type Context[S any] struct {
	State   *S
	agentID string
}

// AgentID returns the raw agent id the running instance was initialized with.
func (c *Context[S]) AgentID() string { return c.agentID }

// Agent is the handle returned by [DefineAgent]. Id is the constructor
// parameter type, which doubles as the agent's type-level identity; S is the
// private state type.
type Agent[Id any, S any] struct{ name string }

// Name returns the agent's wire-level type name.
func (a *Agent[Id, S]) Name() string { return a.name }

// MethodDef is a typed method descriptor: the single source of truth shared by
// the agent-type schema, the implementation binding, and calls from other
// agents. Id is the target agent's identity type, so a descriptor cannot be
// used against another agent's client.
type MethodDef[Id any, In any, Out any] struct {
	name string
	desc string
}

// Name returns the method's wire-level name.
func (m MethodDef[Id, In, Out]) Name() string { return m.name }

// MethodOpt configures a method descriptor.
type MethodOpt func(*methodOpts)

type methodOpts struct{ desc string }

// Desc sets a method's description, surfaced in the agent type metadata.
func Desc(s string) MethodOpt { return func(o *methodOpts) { o.desc = s } }
