// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

// Package identity owns the host calls that address an agent: upsert by
// constructor parameters, create, get by ID, lookup and self binding. Callers
// validate and encode constructors and translate failures.
package identity

import (
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type Stage uint8

const (
	// RPCStage failures carry the host's RPCError.
	RPCStage Stage = iota
	// TypeStage failures are a self binding of another agent type; they carry
	// AgentError.
	TypeStage
)

// Failure retains the host error and the agent type it was about.
type Failure struct {
	Stage      Stage
	TypeName   string
	AgentError common.AgentError
	RPCError   host.RpcError
}

// Connection is a client of one agent.
type Connection struct {
	TypeName string
	// AgentID is the target's ID when the host returned it with the client. A
	// client of Upsert has none: [AgentID] asks the host for it.
	AgentID witTypes.Option[types.AgentId]
	RPC     *host.WasmRpc
}

// Function fields keep native linking independent of generated host imports.
type operations struct {
	upsert   func(string, types.SchemaValueTree) witTypes.Result[*host.WasmRpc, host.RpcError]
	getByID  func(string, types.AgentId) witTypes.Result[*host.WasmRpc, host.RpcError]
	create   func(string, types.SchemaValueTree, host.CreateOptions) witTypes.Result[host.Created, host.RpcError]
	lookup   func(string, types.SchemaValueTree) witTypes.Result[witTypes.Option[types.AgentId], host.RpcError]
	bindSelf func() witTypes.Result[witTypes.Tuple2[types.AgentId, *host.WasmRpc], host.RpcError]
	agentID  func(*host.WasmRpc) witTypes.Result[types.AgentId, host.RpcError]
	typeOf   func(types.AgentId) witTypes.Option[string]
	drop     func(*host.WasmRpc)
}

// Upsert connects to the keyed agent of the constructor parameters; the host
// creates it when it is missing.
func Upsert(name string, ctor types.SchemaValueTree) (Connection, *Failure) {
	return platform.upsertClient(name, ctor)
}

// GetByID connects to the existing agent of type name with the ID. The host
// reports a missing agent, or one of another type, as not-found.
func GetByID(name string, id types.AgentId) (Connection, *Failure) {
	return platform.getClientByID(name, id)
}

// GetAnyByID connects to the existing agent with the ID, of the type its ID
// names.
func GetAnyByID(id types.AgentId) (Connection, *Failure) { return platform.getAnyClientByID(id) }

// Create creates an agent and connects to it.
func Create(name string, ctor types.SchemaValueTree, options host.CreateOptions) (Connection, *Failure) {
	return platform.createClient(name, ctor, options)
}

// Lookup returns the ID of the keyed agent of the constructor parameters, if
// it exists. It never creates the agent.
func Lookup(name string, ctor types.SchemaValueTree) (witTypes.Option[types.AgentId], *Failure) {
	return platform.lookupID(name, ctor)
}

// BindSelf binds the recorded current incarnation and validates its type without creation.
func BindSelf(expectedType string) (Connection, *Failure) { return platform.self(expectedType) }

// AgentID returns the ID of the connection's agent. For a client of Upsert it
// asks the host, which creates the agent when it is missing.
func AgentID(c Connection) (types.AgentId, *Failure) { return platform.connectionID(c) }

// TypeOf returns the name of the deployed agent type that the agent ID's type
// tag names.
func TypeOf(id types.AgentId) (string, bool) {
	name := platform.typeOf(id)
	if name.IsNone() {
		return "", false
	}
	return name.Some(), true
}

func rpcFailure(name string, e host.RpcError) *Failure {
	return &Failure{Stage: RPCStage, TypeName: name, RPCError: e}
}

func (o operations) upsertClient(name string, ctor types.SchemaValueTree) (Connection, *Failure) {
	res := o.upsert(name, ctor)
	if res.IsErr() {
		return Connection{}, rpcFailure(name, res.Err())
	}
	return Connection{TypeName: name, AgentID: witTypes.None[types.AgentId](), RPC: res.Ok()}, nil
}

func (o operations) getClientByID(name string, id types.AgentId) (Connection, *Failure) {
	res := o.getByID(name, id)
	if res.IsErr() {
		return Connection{}, rpcFailure(name, res.Err())
	}
	return Connection{TypeName: name, AgentID: witTypes.Some(id), RPC: res.Ok()}, nil
}

func (o operations) getAnyClientByID(id types.AgentId) (Connection, *Failure) {
	name := o.typeOf(id)
	if name.IsNone() {
		return Connection{}, rpcFailure("", host.MakeRpcErrorNotFound("no deployed agent type has the type tag of the agent ID"))
	}
	return o.getClientByID(name.Some(), id)
}

func (o operations) createClient(name string, ctor types.SchemaValueTree, options host.CreateOptions) (Connection, *Failure) {
	res := o.create(name, ctor, options)
	if res.IsErr() {
		return Connection{}, rpcFailure(name, res.Err())
	}
	created := res.Ok()
	return Connection{TypeName: name, AgentID: witTypes.Some(created.AgentId), RPC: created.Client}, nil
}

func (o operations) lookupID(name string, ctor types.SchemaValueTree) (witTypes.Option[types.AgentId], *Failure) {
	res := o.lookup(name, ctor)
	if res.IsErr() {
		return witTypes.None[types.AgentId](), rpcFailure(name, res.Err())
	}
	return res.Ok(), nil
}

func (o operations) self(expectedType string) (Connection, *Failure) {
	res := o.bindSelf()
	if res.IsErr() {
		return Connection{}, rpcFailure(expectedType, res.Err())
	}
	bound := res.Ok()
	name := o.typeOf(bound.F0)
	if name.IsNone() || name.Some() != expectedType {
		o.drop(bound.F1)
		actual := "an undeployed agent type"
		if name.IsSome() {
			actual = name.Some()
		}
		return Connection{}, &Failure{Stage: TypeStage, TypeName: expectedType,
			AgentError: common.MakeAgentErrorInvalidType("self binding expects " + expectedType + ", identity is " + actual)}
	}
	return Connection{TypeName: expectedType, AgentID: witTypes.Some(bound.F0), RPC: bound.F1}, nil
}

func (o operations) connectionID(c Connection) (types.AgentId, *Failure) {
	if c.AgentID.IsSome() {
		return c.AgentID.Some(), nil
	}
	res := o.agentID(c.RPC)
	if res.IsErr() {
		return types.AgentId{}, rpcFailure(c.TypeName, res.Err())
	}
	return res.Ok(), nil
}
