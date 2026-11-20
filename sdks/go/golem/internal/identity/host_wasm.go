// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

//go:build wasip1

package identity

import (
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var platform = operations{
	upsert: host.WasmRpcUpsert, getByID: host.WasmRpcGetById, create: host.WasmRpcCreate,
	lookup: host.LookupAgentId, bindSelf: host.WasmRpcBindSelf,
	agentID: func(rpc *host.WasmRpc) witTypes.Result[types.AgentId, host.RpcError] { return rpc.AgentId() },
	typeOf: func(id types.AgentId) witTypes.Option[string] {
		found := host.GetAgentTypeByAgentId(id)
		if found.IsNone() {
			return witTypes.None[string]()
		}
		return witTypes.Some(found.Some().AgentType.TypeName)
	},
	drop: func(rpc *host.WasmRpc) { rpc.Drop() },
}
