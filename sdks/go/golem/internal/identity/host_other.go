// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

//go:build !wasip1

package identity

import (
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var platform = operations{
	parse: func(string) witTypes.Result[witTypes.Tuple3[string, types.TypedSchemaValue, witTypes.Option[types.Uuid]], common.AgentError] {
		panic("golem: identity operations are only available on the wasm target")
	},
	construct: func(string, types.SchemaValueTree, witTypes.Option[types.Uuid]) witTypes.Result[string, common.AgentError] {
		panic("golem: identity operations are only available on the wasm target")
	},
	connect: func(string, types.SchemaValueTree, witTypes.Option[types.Uuid], []common.TypedAgentConfigValue) witTypes.Result[*host.WasmRpc, host.RpcError] {
		panic("golem: identity operations are only available on the wasm target")
	},
}
