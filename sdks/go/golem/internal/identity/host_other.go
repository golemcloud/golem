// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

//go:build !wasip1

package identity

import (
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

const offWasm = "golem: identity operations are only available on the wasm target"

var platform = operations{
	upsert: func(string, types.SchemaValueTree) witTypes.Result[*host.WasmRpc, host.RpcError] {
		panic(offWasm)
	},
	getByID: func(string, types.AgentId) witTypes.Result[*host.WasmRpc, host.RpcError] {
		panic(offWasm)
	},
	create: func(string, types.SchemaValueTree, host.CreateOptions) witTypes.Result[host.Created, host.RpcError] {
		panic(offWasm)
	},
	lookup: func(string, types.SchemaValueTree) witTypes.Result[witTypes.Option[types.AgentId], host.RpcError] {
		panic(offWasm)
	},
	bindSelf: func() witTypes.Result[witTypes.Tuple2[types.AgentId, *host.WasmRpc], host.RpcError] {
		panic(offWasm)
	},
	agentID: func(*host.WasmRpc) witTypes.Result[types.AgentId, host.RpcError] {
		panic(offWasm)
	},
	typeOf: func(types.AgentId) witTypes.Option[string] { panic(offWasm) },
	drop:   func(*host.WasmRpc) { panic(offWasm) },
}
