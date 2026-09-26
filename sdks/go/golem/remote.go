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

// Remote agents.
//
// A [RemoteAgent] is a handle to an agent this component CALLS but does not
// implement. It is what a generated cross-component client declares, and what a
// hand-written one should use too:
//
//	var Counter = golem.DeclareRemoteAgent[CounterId]("CounterAgent")
//	var Add     = Counter.Method[AddIn, int64]("add")
//
//	client := Counter.Get(CounterId{Name: "c1"})
//	total := Add.Call(client, AddIn{By: 3})
//
// [DefineAgent] cannot be used for this. It registers an agent type the
// component *provides*: the name is published by discover-agent-types, and an
// agent that is defined but never implemented is a definition error. Declaring
// a call target that way would make the caller advertise an agent it cannot
// run, and fail its own definition check.
//
// A remote declaration therefore records only what a call needs — the target's
// name and the shape of its id — and stays out of everything the component
// publishes about itself.

// Remote is a call target: an agent implemented by some other component.
type Remote[Id any] struct{ name string }

// Name returns the target agent type's name.
func (a *Remote[Id]) Name() string { return a.name }

// DeclareRemoteAgent declares an agent this component calls but does not
// implement. Call it from a package-level var so the declaration is in place
// before the component is invoked.
func DeclareRemoteAgent[Id any](name string) *Remote[Id] {
	return declareRemoteAgentInto[Id](defs, name)
}

// declareRemoteAgentInto is the instance-scoped implementation behind
// [DeclareRemoteAgent].
func declareRemoteAgentInto[Id any](d *definitions, name string) *Remote[Id] {
	idType := reflect.TypeFor[Id]()
	a := &Remote[Id]{name: name}
	if name == "" {
		d.recordErr("", "", "DeclareRemoteAgent requires a non-empty name (Id type %s)", idType)
		return a
	}
	if existing, dup := d.agents[name]; dup {
		if existing.remote {
			d.recordErr(name, "", "remote agent already declared")
		} else {
			d.recordErr(name, "", "%s is defined by this component; call it with its own definition rather than DeclareRemoteAgent", name)
		}
		return a
	}
	if idType.Kind() != reflect.Struct {
		d.recordErr(name, "", "Id must be a struct, got %s", idType)
	}
	// Registered in d.agents so Get can resolve the id fields, but deliberately
	// NOT in d.order: that list is what discover() publishes, and this component
	// does not implement this agent. The Id type is likewise not claimed in
	// idToAgent — that guard exists so two *local* agents cannot share an Id
	// type, which says nothing about a target someone else implements.
	d.agents[name] = &agentEntry{
		name:     name,
		remote:   true,
		idType:   idType,
		idFields: d.structFields(idType),
		methods:  map[string]*methodEntry{},
	}
	return a
}

// Method declares a typed method descriptor on the remote agent, exactly as
// [AgentDefinition.Method] does for a local one. It registers nothing: the
// descriptor is the contract [MethodDef.Call] and friends invoke through.
func (a *Remote[Id]) Method[In any, Out any](name string, opts ...MethodOpt) MethodDef[Id, In, Out] {
	var o methodOpts
	for _, f := range opts {
		f(&o)
	}
	return MethodDef[Id, In, Out]{
		name: name, desc: o.desc, descCount: o.descCount, endpoints: o.endpoints,
		readOnly: o.readOnly, readOnlyCount: o.readOnlyCount, cacheCount: o.cacheCount,
	}
}

// Get returns a client for the remote agent with the given id, creating the
// agent if it does not exist yet — the same contract as
// [AgentDefinition.Get], and it panics on the same failures.
func (a *Remote[Id]) Get(id Id, opts ...ClientOpt) Client[Id] {
	return getClient[Id](defs, a.name, id, opts)
}

// NewPhantom allocates a fresh phantom instance of the remote agent and returns
// a client for it, mirroring [AgentDefinition.NewPhantom].
func (a *Remote[Id]) NewPhantom(id Id) Client[Id] {
	return newPhantomClient[Id](defs, a.name, id)
}
