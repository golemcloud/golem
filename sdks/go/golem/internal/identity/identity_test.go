// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0

package identity

import (
	"reflect"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

var (
	testCtor = types.SchemaValueTree{Root: 3}
	testID   = types.AgentId{Uuid: types.Uuid{HighBits: 0x0000000700008000, LowBits: 0x8000000000000004}}
)

func typeOfCounter(id types.AgentId) witTypes.Option[string] {
	if id == testID {
		return witTypes.Some("Counter")
	}
	return witTypes.None[string]()
}

func TestUpsertCallsTheHostUpsertAndLeavesTheIDToTheHost(t *testing.T) {
	rpc := &host.WasmRpc{}
	var calls []string
	o := operations{
		upsert: func(name string, ctor types.SchemaValueTree) witTypes.Result[*host.WasmRpc, host.RpcError] {
			calls = append(calls, "upsert")
			if name != "Counter" || !reflect.DeepEqual(ctor, testCtor) {
				t.Fatal("upsert arguments changed")
			}
			return witTypes.Ok[*host.WasmRpc, host.RpcError](rpc)
		},
		agentID: func(got *host.WasmRpc) witTypes.Result[types.AgentId, host.RpcError] {
			calls = append(calls, "agent-id")
			if got != rpc {
				t.Fatal("asked another client for its ID")
			}
			return witTypes.Ok[types.AgentId, host.RpcError](testID)
		},
	}
	conn, failure := o.upsertClient("Counter", testCtor)
	if failure != nil || conn.RPC != rpc || conn.TypeName != "Counter" || conn.AgentID.IsSome() {
		t.Fatalf("connection = %+v, failure = %+v", conn, failure)
	}
	id, failure := o.connectionID(conn)
	if failure != nil || id != testID || !reflect.DeepEqual(calls, []string{"upsert", "agent-id"}) {
		t.Fatalf("id = %+v, failure = %+v, calls = %v", id, failure, calls)
	}
}

func TestUpsertAndAgentIDFailuresKeepTheRPCError(t *testing.T) {
	o := operations{
		upsert: func(string, types.SchemaValueTree) witTypes.Result[*host.WasmRpc, host.RpcError] {
			return witTypes.Err[*host.WasmRpc](host.MakeRpcErrorConflict("key has another agent"))
		},
		agentID: func(*host.WasmRpc) witTypes.Result[types.AgentId, host.RpcError] {
			return witTypes.Err[types.AgentId](host.MakeRpcErrorDenied("no view"))
		},
	}
	if _, failure := o.upsertClient("Counter", testCtor); failure == nil || failure.Stage != RPCStage ||
		failure.TypeName != "Counter" || failure.RPCError.Conflict() != "key has another agent" {
		t.Fatalf("upsert failure = %+v", failure)
	}
	conn := Connection{TypeName: "Counter", AgentID: witTypes.None[types.AgentId](), RPC: &host.WasmRpc{}}
	if _, failure := o.connectionID(conn); failure == nil || failure.RPCError.Denied() != "no view" {
		t.Fatalf("agent-id failure = %+v", failure)
	}
}

func TestCreatePassesKeyedConfigAndEnvAndKeepsTheIssuedID(t *testing.T) {
	for _, keyed := range []bool{false, true} {
		options := host.CreateOptions{
			Keyed:       keyed,
			AgentConfig: []common.TypedAgentConfigValue{{Path: []string{"greeting"}}},
			Env:         []witTypes.Tuple2[string, string]{{F0: "MODE", F1: "test"}},
		}
		rpc := &host.WasmRpc{}
		o := operations{
			create: func(name string, ctor types.SchemaValueTree, got host.CreateOptions) witTypes.Result[host.Created, host.RpcError] {
				if name != "Counter" || !reflect.DeepEqual(ctor, testCtor) || !reflect.DeepEqual(got, options) {
					t.Fatalf("create arguments = %q %+v %+v", name, ctor, got)
				}
				return witTypes.Ok[host.Created, host.RpcError](host.Created{Client: rpc, AgentId: testID})
			},
			agentID: func(*host.WasmRpc) witTypes.Result[types.AgentId, host.RpcError] {
				t.Fatal("a created client asked the host for its ID")
				panic("unreachable")
			},
		}
		conn, failure := o.createClient("Counter", testCtor, options)
		if failure != nil || conn.RPC != rpc || conn.AgentID != witTypes.Some(testID) {
			t.Fatalf("keyed=%v: connection = %+v, failure = %+v", keyed, conn, failure)
		}
		if id, failure := o.connectionID(conn); failure != nil || id != testID {
			t.Fatalf("keyed=%v: id = %+v, failure = %+v", keyed, id, failure)
		}
	}
}

func TestKeyedCreateConflictIsTheRPCError(t *testing.T) {
	o := operations{create: func(string, types.SchemaValueTree, host.CreateOptions) witTypes.Result[host.Created, host.RpcError] {
		return witTypes.Err[host.Created](host.MakeRpcErrorConflict("the key is current"))
	}}
	_, failure := o.createClient("Counter", testCtor, host.CreateOptions{Keyed: true})
	if failure == nil || failure.TypeName != "Counter" || failure.RPCError.Tag() != host.RpcErrorConflict ||
		failure.RPCError.Conflict() != "the key is current" {
		t.Fatalf("failure = %+v", failure)
	}
}

func TestGetByIDPassesTheTypeAndIDAndKeepsNotFound(t *testing.T) {
	for _, missing := range []bool{false, true} {
		rpc := &host.WasmRpc{}
		o := operations{getByID: func(name string, id types.AgentId) witTypes.Result[*host.WasmRpc, host.RpcError] {
			if name != "Counter" || id != testID {
				t.Fatalf("get-by-id arguments = %q %+v", name, id)
			}
			if missing {
				return witTypes.Err[*host.WasmRpc](host.MakeRpcErrorNotFound("no agent of Counter has the ID"))
			}
			return witTypes.Ok[*host.WasmRpc, host.RpcError](rpc)
		}}
		conn, failure := o.getClientByID("Counter", testID)
		if missing {
			if failure == nil || failure.RPCError.Tag() != host.RpcErrorNotFound {
				t.Fatalf("failure = %+v", failure)
			}
			continue
		}
		if failure != nil || conn.RPC != rpc || conn.AgentID != witTypes.Some(testID) {
			t.Fatalf("connection = %+v, failure = %+v", conn, failure)
		}
	}
}

func TestGetAnyByIDReadsTheTypeFromTheTag(t *testing.T) {
	var asked []string
	o := operations{
		typeOf: typeOfCounter,
		getByID: func(name string, _ types.AgentId) witTypes.Result[*host.WasmRpc, host.RpcError] {
			asked = append(asked, name)
			return witTypes.Ok[*host.WasmRpc, host.RpcError](&host.WasmRpc{})
		},
	}
	if conn, failure := o.getAnyClientByID(testID); failure != nil || conn.TypeName != "Counter" {
		t.Fatalf("connection = %+v, failure = %+v", conn, failure)
	}
	if _, failure := o.getAnyClientByID(types.AgentId{}); failure == nil || failure.RPCError.Tag() != host.RpcErrorNotFound {
		t.Fatalf("an unknown tag gave %+v", failure)
	}
	if !reflect.DeepEqual(asked, []string{"Counter"}) {
		t.Fatalf("get-by-id calls = %v", asked)
	}
}

func TestLookupNeverConnects(t *testing.T) {
	for _, found := range []witTypes.Option[types.AgentId]{witTypes.None[types.AgentId](), witTypes.Some(testID)} {
		o := operations{lookup: func(name string, ctor types.SchemaValueTree) witTypes.Result[witTypes.Option[types.AgentId], host.RpcError] {
			if name != "Counter" || !reflect.DeepEqual(ctor, testCtor) {
				t.Fatal("lookup arguments changed")
			}
			return witTypes.Ok[witTypes.Option[types.AgentId], host.RpcError](found)
		}}
		got, failure := o.lookupID("Counter", testCtor)
		if failure != nil || got != found {
			t.Fatalf("lookup = %+v, failure = %+v", got, failure)
		}
	}
	o := operations{lookup: func(string, types.SchemaValueTree) witTypes.Result[witTypes.Option[types.AgentId], host.RpcError] {
		return witTypes.Err[witTypes.Option[types.AgentId]](host.MakeRpcErrorDenied("denied"))
	}}
	if _, failure := o.lookupID("Counter", testCtor); failure == nil || failure.RPCError.Denied() != "denied" {
		t.Fatalf("failure = %+v", failure)
	}
}

func TestSelfBindingPreservesRecordedIdentityAndRefusesWrongTypes(t *testing.T) {
	for _, failAt := range []string{"", "host", "type", "undeployed"} {
		t.Run(failAt, func(t *testing.T) {
			rpc := &host.WasmRpc{}
			bound := testID
			if failAt == "undeployed" {
				bound = types.AgentId{}
			}
			drops := 0
			o := operations{
				bindSelf: func() witTypes.Result[witTypes.Tuple2[types.AgentId, *host.WasmRpc], host.RpcError] {
					if failAt == "host" {
						return witTypes.Err[witTypes.Tuple2[types.AgentId, *host.WasmRpc]](host.MakeRpcErrorDenied("ephemeral owner"))
					}
					return witTypes.Ok[witTypes.Tuple2[types.AgentId, *host.WasmRpc], host.RpcError](
						witTypes.Tuple2[types.AgentId, *host.WasmRpc]{F0: bound, F1: rpc})
				},
				typeOf: typeOfCounter,
				drop: func(got *host.WasmRpc) {
					if got != rpc {
						t.Fatal("dropped another resource")
					}
					drops++
				},
			}
			expected := "Counter"
			if failAt == "type" {
				expected = "Other"
			}
			got, failure := o.self(expected)
			switch failAt {
			case "":
				if failure != nil || got.AgentID != witTypes.Some(testID) || got.RPC != rpc || drops != 0 {
					t.Fatal("lost bound identity or resource")
				}
			case "host":
				if failure == nil || failure.Stage != RPCStage || failure.RPCError.Denied() != "ephemeral owner" || drops != 0 {
					t.Fatal("lost host refusal")
				}
			case "type":
				if failure == nil || failure.Stage != TypeStage ||
					failure.AgentError.InvalidType() != "self binding expects Other, identity is Counter" || drops != 1 {
					t.Fatal("lost typed refusal or leaked resource")
				}
			case "undeployed":
				if failure == nil || failure.Stage != TypeStage || drops != 1 {
					t.Fatal("bound an undeployed type or leaked resource")
				}
			}
		})
	}
}
