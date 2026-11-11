// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

// Package identity owns host identity resolution and RPC connection sequencing.
// Callers validate and encode constructors and translate stage-specific failures.
package identity

import (
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type Stage uint8

const (
	ParseStage Stage = iota
	ConstructStage
	ConnectStage
)

// Failure retains the host error and the operation that failed. Only RPC
// connection failures carry RPCError; the other stages carry AgentError.
type Failure struct {
	Stage      Stage
	TypeName   string
	AgentError common.AgentError
	RPCError   host.RpcError
}

type Parsed struct {
	TypeName    string
	Constructor types.TypedSchemaValue
	PhantomID   witTypes.Option[types.Uuid]
}

type Connection struct {
	AgentID   string
	TypeName  string
	PhantomID witTypes.Option[types.Uuid]
	RPC       *host.WasmRpc
}

// Function fields keep native linking independent of generated host imports.
type operations struct {
	parse     func(string) witTypes.Result[witTypes.Tuple3[string, types.TypedSchemaValue, witTypes.Option[types.Uuid]], common.AgentError]
	construct func(string, types.SchemaValueTree, witTypes.Option[types.Uuid]) witTypes.Result[string, common.AgentError]
	connect   func(string, types.SchemaValueTree, witTypes.Option[types.Uuid], []common.TypedAgentConfigValue) witTypes.Result[*host.WasmRpc, host.RpcError]
}

func Parse(agentID string) (Parsed, *Failure) { return platform.parseID(agentID) }

func Render(name string, ctor types.SchemaValueTree, phantom witTypes.Option[types.Uuid]) (string, *Failure) {
	return platform.render(name, ctor, phantom)
}

// Construct resolves the identity before connecting, so invalid constructor
// parameters fail before RPC creation and retain their agent-error channel.
func Construct(name string, ctor types.SchemaValueTree, phantom witTypes.Option[types.Uuid], config []common.TypedAgentConfigValue) (Connection, *Failure) {
	return platform.constructClient(name, ctor, phantom, config)
}

// Bind connects an already parsed identity, without rendering it again.
func Bind(agentID string, parsed Parsed, config []common.TypedAgentConfigValue) (Connection, *Failure) {
	return platform.bind(agentID, parsed, config)
}

// Connect parses an existing identity and connects to exactly that target.
func Connect(agentID string) (Connection, *Failure) { return platform.connectID(agentID) }

func (o operations) parseID(agentID string) (Parsed, *Failure) {
	res := o.parse(agentID)
	if res.IsErr() {
		return Parsed{}, &Failure{Stage: ParseStage, AgentError: res.Err()}
	}
	p := res.Ok()
	return Parsed{TypeName: p.F0, Constructor: p.F1, PhantomID: p.F2}, nil
}

func (o operations) render(name string, ctor types.SchemaValueTree, phantom witTypes.Option[types.Uuid]) (string, *Failure) {
	res := o.construct(name, ctor, phantom)
	if res.IsErr() {
		return "", &Failure{Stage: ConstructStage, TypeName: name, AgentError: res.Err()}
	}
	return res.Ok(), nil
}

func (o operations) constructClient(name string, ctor types.SchemaValueTree, phantom witTypes.Option[types.Uuid], config []common.TypedAgentConfigValue) (Connection, *Failure) {
	agentID, failure := o.render(name, ctor, phantom)
	if failure != nil {
		return Connection{}, failure
	}
	return o.bind(agentID, Parsed{TypeName: name, Constructor: types.TypedSchemaValue{Value: ctor}, PhantomID: phantom}, config)
}

func (o operations) bind(agentID string, parsed Parsed, config []common.TypedAgentConfigValue) (Connection, *Failure) {
	res := o.connect(parsed.TypeName, parsed.Constructor.Value, parsed.PhantomID, config)
	if res.IsErr() {
		return Connection{}, &Failure{Stage: ConnectStage, TypeName: parsed.TypeName, RPCError: res.Err()}
	}
	return Connection{AgentID: agentID, TypeName: parsed.TypeName, PhantomID: parsed.PhantomID, RPC: res.Ok()}, nil
}

func (o operations) connectID(agentID string) (Connection, *Failure) {
	parsed, failure := o.parseID(agentID)
	if failure != nil {
		return Connection{}, failure
	}
	return o.bind(agentID, parsed, nil)
}
