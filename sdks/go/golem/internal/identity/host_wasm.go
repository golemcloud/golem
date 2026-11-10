// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

//go:build wasip1

package identity

import host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"

var platform = operations{parse: host.ParseAgentId, construct: host.MakeAgentId, connect: host.WasmRpcCreate}
