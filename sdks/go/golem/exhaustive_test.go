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
	"go/ast"
	"go/parser"
	"go/token"
	"path/filepath"
	"strings"
	"testing"
)

// Go switches over the bindings' tags are not exhaustive: a case the WIT gains
// falls into a default, or into nothing, at run time. Each switch below is
// pinned to the number of tags the generated bindings declare, counted from
// their source, so a new case fails here until the switch handles it.

// tagCount counts the uint8 tag constants named prefix<Case> in a generated
// bindings package.
func tagCount(t *testing.T, pkg, prefix string) int {
	t.Helper()
	file, err := parser.ParseFile(token.NewFileSet(), filepath.Join("internal/wit", pkg, "wit_bindings.go"), nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	n := 0
	for _, decl := range file.Decls {
		gen, ok := decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.CONST {
			continue
		}
		for _, spec := range gen.Specs {
			vs := spec.(*ast.ValueSpec)
			if id, ok := vs.Type.(*ast.Ident); !ok || id.Name != "uint8" {
				continue
			}
			for _, name := range vs.Names {
				if rest, ok := strings.CutPrefix(name.Name, prefix); ok && rest != "" && strings.ToUpper(rest[:1]) == rest[:1] {
					n++
				}
			}
		}
	}
	return n
}

func TestWitTagSwitchesCoverEveryCase(t *testing.T) {
	for _, c := range []struct {
		pkg, prefix, where string
		handled            int
	}{
		// reflection.go canonicalFields: scalar, optional scalar, list, map.
		{"golem_tool_common", "OptionShape", "reflection.go canonicalFields", 4},
		// reflection.go canonicalFields: bool and count flags.
		{"golem_tool_common", "FlagShape", "reflection.go canonicalFields", 2},
		// toolcall.go toolCallErrorFromWit, outer switch.
		{"golem_core_types", "ToolRpcError", "toolcall.go toolCallErrorFromWit", 7},
		// toolcall.go toolCallErrorFromWit, remote tool error.
		{"golem_core_types", "ToolError", "toolcall.go toolCallErrorFromWit", 6},
		// client.go agentErrorToGo.
		{"golem_agent_common", "AgentError", "client.go agentErrorToGo", 5},
		// rpc.go rpcErrorToGo.
		{"golem_agent_host", "RpcError", "rpc.go rpcErrorToGo", 5},
		// principal.go principalFromWit: an unknown principal must never pass as anonymous.
		{"golem_agent_common", "Principal", "principal.go principalFromWit", 4},
		// agentops.go agentOperationErrorFromWit.
		{"golem_api_host", "AgentOperationError", "agentops.go agentOperationErrorFromWit", 2},
		// promise.go webhookErrorToGo.
		{"golem_agent_host", "WebhookError", "promise.go webhookErrorToGo", 2},
		// oplog/oplog_wasm.go readErrorFromWit.
		{"golem_api_oplog", "OplogReadError", "oplog readErrorFromWit", 2},
		// websocket/websocket.go wsError.
		{"golem_websocket_client", "Error", "websocket wsError", 7},
		// durablestreams/reader.go errorFromWire converts the kind by position.
		{"golem_agent_durable_streams", "DurableStreamErrorKind", "durablestreams ErrorKind", 14},
		// rdbms/postgres: pgError; tagName, and decodeFlat with decodeLazy.
		{"golem_rdbms_postgres", "Error", "postgres pgError", 6},
		{"golem_rdbms_postgres", "DbValue", "postgres tagName, decodeFlat and decodeLazy", 45},
		// rdbms/mysql: its error mapping; tagName and decodeValue.
		{"golem_rdbms_mysql", "Error", "mysql error mapping", 6},
		{"golem_rdbms_mysql", "DbValue", "mysql tagName and decodeValue", 36},
	} {
		if got := tagCount(t, c.pkg, c.prefix); got != c.handled {
			t.Errorf("%s declares %d %s cases, %s handles %d: handle the new case and move the count",
				c.pkg, got, c.prefix, c.where, c.handled)
		}
	}
}
