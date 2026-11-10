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

func TestConstructSequenceAndFailures(t *testing.T) {
	for _, phantom := range []witTypes.Option[types.Uuid]{witTypes.None[types.Uuid](), witTypes.Some(types.Uuid{HighBits: 1, LowBits: 2})} {
		for _, failAt := range []string{"", "construct", "connect"} {
			t.Run(failAt+"/phantom="+map[bool]string{true: "some", false: "none"}[phantom.IsSome()], func(t *testing.T) {
				ctor := types.SchemaValueTree{Root: 3}
				config := []common.TypedAgentConfigValue{{}}
				var calls []string
				o := operations{
					construct: func(name string, value types.SchemaValueTree, p witTypes.Option[types.Uuid]) witTypes.Result[string, common.AgentError] {
						calls = append(calls, "construct")
						if name != "Counter" || !reflect.DeepEqual(value, ctor) || p != phantom {
							t.Fatal("construction arguments changed")
						}
						if failAt == "construct" {
							return witTypes.Err[string](common.MakeAgentErrorInvalidInput("bad constructor"))
						}
						return witTypes.Ok[string, common.AgentError]("host-resolved-id")
					},
					connect: func(name string, value types.SchemaValueTree, p witTypes.Option[types.Uuid], cfg []common.TypedAgentConfigValue) witTypes.Result[*host.WasmRpc, host.RpcError] {
						calls = append(calls, "connect")
						if name != "Counter" || !reflect.DeepEqual(value, ctor) || p != phantom || !reflect.DeepEqual(cfg, config) {
							t.Fatal("connection arguments changed")
						}
						if failAt == "connect" {
							return witTypes.Err[*host.WasmRpc](host.MakeRpcErrorDenied("denied"))
						}
						return witTypes.Ok[*host.WasmRpc, host.RpcError](nil)
					},
				}
				got, failure := o.constructClient("Counter", ctor, phantom, config)
				switch failAt {
				case "construct":
					if failure == nil || failure.Stage != ConstructStage || failure.TypeName != "Counter" || failure.AgentError.InvalidInput() != "bad constructor" {
						t.Fatal("lost construction failure")
					}
					if !reflect.DeepEqual(calls, []string{"construct"}) {
						t.Fatal("connected after construction failed")
					}
				case "connect":
					if failure == nil || failure.Stage != ConnectStage || failure.TypeName != "Counter" || failure.RPCError.Denied() != "denied" {
						t.Fatal("lost connection failure")
					}
				default:
					if failure != nil || got.AgentID != "host-resolved-id" || got.TypeName != "Counter" || got.PhantomID != phantom {
						t.Fatal("lost host identity")
					}
				}
				if failAt != "construct" && !reflect.DeepEqual(calls, []string{"construct", "connect"}) {
					t.Fatal("incorrect host call order")
				}
			})
		}
	}
}

func TestConnectSequenceAndFailures(t *testing.T) {
	for _, failAt := range []string{"", "parse", "connect"} {
		t.Run(failAt, func(t *testing.T) {
			parsed := Parsed{TypeName: "Counter", Constructor: types.TypedSchemaValue{Value: types.SchemaValueTree{Root: 7}}, PhantomID: witTypes.Some(types.Uuid{HighBits: 4, LowBits: 5})}
			var calls []string
			o := operations{
				parse: func(agentID string) witTypes.Result[witTypes.Tuple3[string, types.TypedSchemaValue, witTypes.Option[types.Uuid]], common.AgentError] {
					calls = append(calls, "parse")
					if agentID != "existing-id" {
						t.Fatal("parse input changed")
					}
					if failAt == "parse" {
						return witTypes.Err[witTypes.Tuple3[string, types.TypedSchemaValue, witTypes.Option[types.Uuid]]](common.MakeAgentErrorInvalidAgentId("bad id"))
					}
					return witTypes.Ok[witTypes.Tuple3[string, types.TypedSchemaValue, witTypes.Option[types.Uuid]], common.AgentError](witTypes.Tuple3[string, types.TypedSchemaValue, witTypes.Option[types.Uuid]]{F0: parsed.TypeName, F1: parsed.Constructor, F2: parsed.PhantomID})
				},
				connect: func(name string, value types.SchemaValueTree, p witTypes.Option[types.Uuid], cfg []common.TypedAgentConfigValue) witTypes.Result[*host.WasmRpc, host.RpcError] {
					calls = append(calls, "connect")
					if name != parsed.TypeName || !reflect.DeepEqual(value, parsed.Constructor.Value) || p != parsed.PhantomID || cfg != nil {
						t.Fatal("parsed target changed")
					}
					if failAt == "connect" {
						return witTypes.Err[*host.WasmRpc](host.MakeRpcErrorNotFound("missing"))
					}
					return witTypes.Ok[*host.WasmRpc, host.RpcError](nil)
				},
			}
			got, failure := o.connectID("existing-id")
			switch failAt {
			case "parse":
				if failure == nil || failure.Stage != ParseStage || failure.AgentError.InvalidAgentId() != "bad id" {
					t.Fatal("lost parse failure")
				}
				if !reflect.DeepEqual(calls, []string{"parse"}) {
					t.Fatal("connected after parse failed")
				}
			case "connect":
				if failure == nil || failure.Stage != ConnectStage || failure.TypeName != "Counter" || failure.RPCError.NotFound() != "missing" {
					t.Fatal("lost bind failure")
				}
			default:
				if failure != nil || got.AgentID != "existing-id" || got.TypeName != parsed.TypeName || got.PhantomID != parsed.PhantomID {
					t.Fatal("lost parsed identity")
				}
			}
			if failAt != "parse" && !reflect.DeepEqual(calls, []string{"parse", "connect"}) {
				t.Fatal("bind rendered an existing identity or connected before parsing")
			}
		})
	}
}

func TestBindUsesParsedTargetAndConfig(t *testing.T) {
	config := []common.TypedAgentConfigValue{{}}
	parsed := Parsed{TypeName: "Counter", Constructor: types.TypedSchemaValue{Value: types.SchemaValueTree{Root: 2}}}
	o := operations{connect: func(name string, value types.SchemaValueTree, p witTypes.Option[types.Uuid], cfg []common.TypedAgentConfigValue) witTypes.Result[*host.WasmRpc, host.RpcError] {
		if name != parsed.TypeName || !reflect.DeepEqual(value, parsed.Constructor.Value) || p != parsed.PhantomID || !reflect.DeepEqual(cfg, config) {
			t.Fatal("bind arguments changed")
		}
		return witTypes.Ok[*host.WasmRpc, host.RpcError](nil)
	}}
	got, failure := o.bind("existing-id", parsed, config)
	if failure != nil || got.AgentID != "existing-id" {
		t.Fatal("bind failed")
	}
}
