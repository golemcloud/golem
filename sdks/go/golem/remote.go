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
	"reflect"
)

// Agent clients.
//
// An agent client definition lets a component call agents whose Go definition
// it does not share. It comes in two forms.
//
// A method-only client declares only methods, so it can call any existing
// agent that has them, whatever its type — a shape many agent types share:
//
//	type Pingable struct{}
//
//	var Pinger = golem.DefineAgentClient[Pingable]()
//	var Ping   = Pinger.Method[golem.Unit, string]("ping")
//
//	c, err := Pinger.Bind(agentID)
//	reply, err := Ping.Call(c, golem.Unit{})
//
// A full client also declares the target type's name, constructor (Id) and
// lifecycle, so it creates and addresses instances like the agent's own
// definition. Generated guest clients use it:
//
//	var Counter = golem.DefineFullAgentClient[CounterId]("CounterAgent", golem.AgentClientSpec{})
//	var Add     = Counter.Method[AddIn, int64]("add")
//
//	c, err := Counter.Get(CounterId{Name: "c1"})
//	total, err := Add.Call(c, AddIn{By: 3})
//
// Neither is published by the component: [DefineAgent] declares an agent the
// component provides, which is a different thing.

// AgentClientSpec describes the target of a full agent client.
type AgentClientSpec struct {
	// Mode is the target's lifecycle. An ephemeral target has no durable
	// identity, so its client offers NewPhantom only.
	Mode Mode
}

// AgentClient is a method-only agent client definition; Shape is its identity
// type, which its methods and clients carry.
type AgentClient[Shape any] struct{}

// DefineAgentClient declares a method-only agent client.
func DefineAgentClient[Shape any]() *AgentClient[Shape] { return &AgentClient[Shape]{} }

// Method declares a method the client calls.
func (a *AgentClient[Shape]) Method[In any, Out any](name string, opts ...MethodOpt) MethodDef[Shape, In, Out] {
	return newMethodDef[Shape, In, Out](name, opts)
}

// Bind addresses the existing agent agentID names, of any type. Binding checks
// nothing about the target, which this definition does not describe; a
// mismatch is reported by the host or when the result is decoded. Binding does
// not create the agent, but the first call to a durable identity does.
func (a *AgentClient[Shape]) Bind(agentID string) (Client[Shape], error) {
	return bindClient[Shape](agentID)
}

// FullAgentClient is a full agent client definition: the target type's name,
// constructor (Id), lifecycle and configuration (Cfg).
type FullAgentClient[Id any, Cfg any] struct {
	name string
	mode Mode
}

// DefineFullAgentClient declares a full agent client for the agent type name.
// Call it from a package-level var.
func DefineFullAgentClient[Id any](name string, spec AgentClientSpec) *FullAgentClient[Id, NoConfig] {
	return defineFullAgentClientInto[Id, NoConfig](defs, name, spec)
}

// DefineConfiguredFullAgentClient declares a full agent client whose target
// takes configuration Cfg, so a caller can pass typed creation-time overrides
// with [WithConfig].
func DefineConfiguredFullAgentClient[Id any, Cfg any](name string, spec AgentClientSpec) *FullAgentClient[Id, Cfg] {
	return defineFullAgentClientInto[Id, Cfg](defs, name, spec)
}

func defineFullAgentClientInto[Id any, Cfg any](d *definitions, name string, spec AgentClientSpec) *FullAgentClient[Id, Cfg] {
	idType := reflect.TypeFor[Id]()
	a := &FullAgentClient[Id, Cfg]{name: name, mode: spec.Mode}
	if name == "" {
		d.RecordErr("", "", "DefineFullAgentClient requires a non-empty name (Id type %s)", idType)
		return a
	}
	if existing, dup := d.agents[name]; dup {
		if existing.remote {
			d.RecordErr(name, "", "agent client already defined")
		} else {
			d.RecordErr(name, "", "%s is defined by this component; call it with its own definition rather than a client definition", name)
		}
		return a
	}
	if idType.Kind() != reflect.Struct {
		d.RecordErr(name, "", "Id must be a struct, got %s", idType)
	}
	// Registered in d.agents so Get can resolve the id fields and config
	// overrides, but deliberately NOT in d.order: that list is what discover()
	// publishes, and this component does not implement this agent. The Id type
	// is likewise not claimed in idToAgent — that guard exists so two *local*
	// agents cannot share an Id type, which says nothing about a target someone
	// else implements.
	e := &agentEntry{
		name:     name,
		remote:   true,
		mode:     spec.Mode.toWit(),
		idType:   idType,
		idFields: d.StructFields(idType),
		methods:  map[string]*methodEntry{},
	}
	d.agents[name] = e
	flattenConfigStruct(d, e, name, reflect.TypeFor[Cfg]())
	return a
}

// Name returns the target agent type's name.
func (a *FullAgentClient[Id, Cfg]) Name() string { return a.name }

// Method declares a typed method descriptor on the target, exactly as
// [AgentDefinition.Method] does for a local agent.
func (a *FullAgentClient[Id, Cfg]) Method[In any, Out any](name string, opts ...MethodOpt) MethodDef[Id, In, Out] {
	return newMethodDef[Id, In, Out](name, opts)
}

// Get returns a client for the agent with the given id, creating it if it does
// not exist yet. Unlike [AgentDefinition.Get] it returns an error rather than
// panicking when the host cannot resolve the target: the target is another
// component's agent, which may not be deployed. Misuse — an override that does
// not match the declared config — still panics.
func (a *FullAgentClient[Id, Cfg]) Get(id Id, opts ...ClientOpt) (Client[Id], error) {
	return getClient[Id](defs, a.name, id, opts)
}

// MustGet is [FullAgentClient.Get] that panics when the host cannot resolve
// the target.
func (a *FullAgentClient[Id, Cfg]) MustGet(id Id, opts ...ClientOpt) Client[Id] {
	return Must(a.Get(id, opts...))
}

// NewPhantom allocates a fresh phantom instance and returns a client for it,
// with the error contract of [FullAgentClient.Get].
func (a *FullAgentClient[Id, Cfg]) NewPhantom(id Id, opts ...ClientOpt) (Client[Id], error) {
	return newPhantomClient[Id](defs, a.name, id, opts)
}

// MustNewPhantom is [FullAgentClient.NewPhantom] that panics when the host
// cannot resolve the target.
func (a *FullAgentClient[Id, Cfg]) MustNewPhantom(id Id, opts ...ClientOpt) Client[Id] {
	return Must(a.NewPhantom(id, opts...))
}

// Bind addresses an existing agent by its id, which must name this client's
// agent type and carry a constructor of shape Id.
func (a *FullAgentClient[Id, Cfg]) Bind(agentID string) (Client[Id], error) {
	return bindTypedClient[Id](defs, a.name, agentID)
}

// AgentID renders the identity of the instance id (and phantom, if any)
// names, without creating it.
func (a *FullAgentClient[Id, Cfg]) AgentID(id Id, phantom Option[UUID]) (string, error) {
	return makeTypedAgentID[Id](defs, a.name, id, phantom)
}
