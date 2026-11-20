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
//	c, err := Pinger.GetByID(agentID)
//	reply, err := Ping.Call(c, golem.Unit{})
//
// A full client also declares the target type's name, constructor (Id) and
// lifecycle, so it creates and addresses instances like the agent's own
// definition. Generated guest clients use it:
//
//	var Counter = golem.DefineFullAgentClient[CounterID]("CounterAgent", golem.AgentClientSpec{})
//	var Add     = Counter.Method[AddIn, int64]("add")
//
//	c, err := Counter.Upsert(CounterID{Name: "c1"})
//	total, err := Add.Call(c, AddIn{By: 3})
//
// Neither is published by the component: [DefineAgent] declares an agent the
// component provides, which is a different thing.

// AgentClientSpec describes the target of a full agent client.
type AgentClientSpec struct {
	// Mode is the target's lifecycle. An ephemeral target has no durable
	// identity: each invocation runs on a new one-shot instance, so its client
	// is made with Upsert only.
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

// GetByID addresses the existing agent with the ID, of the type its ID names.
// It checks nothing else about the target, which this definition does not
// describe; a mismatch is reported by the host or when the result is decoded.
// It never creates the agent: an unknown ID is a [RemoteCallError] of kind
// [RemoteNotFound].
func (a *AgentClient[Shape]) GetByID(id AgentID) (Client[Shape], error) {
	return getAnyClientByID[Shape](id)
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
// takes configuration Cfg, so a caller can pass typed overrides to
// [FullAgentClient.Create] with [FullAgentClient.WithConfig].
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
	// Registered in d.agents so Upsert and Create can resolve the id fields and config
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

// Upsert returns a client for the keyed agent of the constructor parameters
// id, which the host creates when it is missing. Unlike
// [AgentDefinition.Upsert] it returns an error rather than panicking when the
// host refuses it: the target is another component's agent, which may not be
// deployed.
func (a *FullAgentClient[Id, Cfg]) Upsert(id Id) (Client[Id], error) {
	return upsertClient[Id](defs, a.name, id)
}

// MustUpsert is [FullAgentClient.Upsert] that panics when the host refuses it.
func (a *FullAgentClient[Id, Cfg]) MustUpsert(id Id) Client[Id] {
	return Must(a.Upsert(id))
}

// Create creates an agent with the constructor parameters id, as
// [AgentDefinition.Create] does for a local agent.
func (a *FullAgentClient[Id, Cfg]) Create(id Id, opts ...CreateOpt) (Client[Id], error) {
	return createClient[Id](defs, a.name, id, opts)
}

// MustCreate is [FullAgentClient.Create] that panics on a failure.
func (a *FullAgentClient[Id, Cfg]) MustCreate(id Id, opts ...CreateOpt) Client[Id] {
	return Must(a.Create(id, opts...))
}

// GetByID addresses the existing agent of this client's type with the ID, as
// [AgentDefinition.GetByID] does for a local agent.
func (a *FullAgentClient[Id, Cfg]) GetByID(id AgentID) (Client[Id], error) {
	return getClientByID[Id](defs, a.name, id)
}

// MustGetByID is [FullAgentClient.GetByID] that panics on a failure.
func (a *FullAgentClient[Id, Cfg]) MustGetByID(id AgentID) Client[Id] {
	return Must(a.GetByID(id))
}

// LookupID returns the ID of the keyed agent of the constructor parameters id,
// or None when no such agent exists. It never creates the agent.
func (a *FullAgentClient[Id, Cfg]) LookupID(id Id) (Option[AgentID], error) {
	return lookupID[Id](defs, a.name, id)
}

// BindSelf binds the current durable incarnation without creating it or waiting for initialization.
//
// Use this to schedule work on self from inside the constructor. Upsert reuses an
// initialized agent without rerunning its constructor, but waits for an unfinished
// constructor. Calling Upsert on self from that constructor would therefore deadlock.
//
// Scheduled work runs after construction finishes. This does not make awaited self-calls
// reentrant or bypass method permissions. A different agent type or an ephemeral owner
// returns an error. Replay preserves the recorded incarnation.
func (a *FullAgentClient[Id, Cfg]) BindSelf() (Client[Id], error) {
	return bindSelfClient[Id](a.name)
}
