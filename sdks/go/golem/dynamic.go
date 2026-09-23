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
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/schema"
)

// Dynamic and method-only clients.
//
// These are the surfaces for callers that already hold schema-native values, or
// that own their own compile-time types and only need an existing identity to
// aim them at. Neither retains a deployment snapshot, so neither claims the
// validation a reflected client performs: the host still authorises the call
// and validates the target-side input.
//
//	// Already-packed values, no schema authority retained.
//	client, err := golem.BindAgentID(id)
//	out, err := client.InvokeDynamic("greet", input)
//
//	// Caller-owned types against an existing identity.
//	greeting, err := golem.Invoke[GreetIn, string](client, "greet", GreetIn{Greeting: "hi"})

// RawAgentID is an agent identity taken apart by the host, with the
// constructor left as a typed value rather than decoded into a Go type — which
// is the point: a dynamic caller has no such type. The typed counterpart is
// [ParseAgentID].
type RawAgentID struct {
	// AgentType is the type name the identity names.
	AgentType string
	// Constructor carries the caller-supplied constructor fields. The host
	// injects principal fields separately, so a principal-scoped agent can be
	// rebuilt from an identity returned by invocation metadata.
	Constructor TypedValue
	// Phantom is the phantom instance this identity addresses, if any.
	Phantom Option[UUID]
}

// ConstructorJSON reads the constructor fields as canonical JSON.
func (p RawAgentID) ConstructorJSON() (any, error) { return p.Constructor.JSON() }

// DynamicAgentClient invokes an existing agent with values the caller packed
// itself. It keeps no schema, so the caller owns packing and validation policy.
type DynamicAgentClient struct {
	agentID string
	parsed  RawAgentID
	rpc     reflectedRPC
}

// AgentID returns the identity this client is bound to.
func (c *DynamicAgentClient) AgentID() string { return c.agentID }

// Parsed returns what the host made of the identity.
func (c *DynamicAgentClient) Parsed() RawAgentID { return c.parsed }

// InvokeDynamic calls a method with an already-packed parameter tree and
// returns the raw result, which is none for a method that returns nothing.
func (c *DynamicAgentClient) InvokeDynamic(method string, input types.SchemaValueTree) (Option[types.SchemaValueTree], error) {
	tree, has, err := c.rpc.invokeAndAwait(method, input)
	if err != nil {
		return None[types.SchemaValueTree](), err
	}
	if !has {
		return None[types.SchemaValueTree](), nil
	}
	return Some(tree), nil
}

// InvokeJSON calls a method with arguments packed against a schema the caller
// supplies, which is the shape an infrastructure transport already holds.
func (c *DynamicAgentClient) InvokeJSON(
	method string, ref schema.Ref, params []schema.Parameter, args map[string]any,
) (Option[types.SchemaValueTree], error) {
	input, err := ref.PackParameters(params, args)
	if err != nil {
		return None[types.SchemaValueTree](), fmt.Errorf("golem: %s: %w", method, err)
	}
	return c.InvokeDynamic(method, input)
}

// Invoke calls a method using the caller's own compile-time types. The target's
// type name and constructor are not checked — this client binds an identity, it
// does not claim to know what is behind it — so a mismatch surfaces as a
// decoding failure or as the host rejecting the input.
func Invoke[In any, Out any](c *DynamicAgentClient, method string, in In) (Out, error) {
	var zero Out
	inFields, outCodec, err := localMethodCodecs[In, Out]()
	if err != nil {
		return zero, err
	}
	input := encodeParams(inFields, valueOf(&in))

	tree, err := c.InvokeDynamic(method, input)
	if err != nil {
		return zero, err
	}
	if outCodec == nil {
		if tree.IsSome() {
			return zero, fmt.Errorf("golem: %s returned a value but Out is golem.Unit", method)
		}
		return zero, nil
	}
	value, present := tree.Get()
	if !present {
		return zero, fmt.Errorf("golem: %s returned nothing but Out is %T", method, zero)
	}
	out := newOf[Out]()
	d := decoder{nodes: value.ValueNodes}
	if err := outCodec.decode(&d, out, value.Root); err != nil {
		return zero, fmt.Errorf("golem: %s returned an unreadable result: %w", method, err)
	}
	return out.Interface().(Out), nil
}

// DynamicToolClient invokes a tool by name and command path, with values the
// caller packed itself.
type DynamicToolClient struct {
	toolName string
	rpc      reflectedToolRPC
}

// ToolName returns the tool this client is bound to.
func (c *DynamicToolClient) ToolName() string { return c.toolName }

// InvokeDynamic runs a command with an already-packed input and returns the raw
// result, which is none for a command that produces nothing.
func (c *DynamicToolClient) InvokeDynamic(path []string, input TypedValue) (Option[TypedValue], error) {
	out, has, err := c.rpc.invokeAndAwait(path, input.wit)
	if err != nil {
		return None[TypedValue](), err
	}
	if !has {
		return None[TypedValue](), nil
	}
	return Some(TypedValue{wit: out}), nil
}

// localMethodCodecs compiles the caller's own input and output types. A Unit
// output means the method returns nothing.
func localMethodCodecs[In any, Out any]() ([]fieldInfo, *codec, error) {
	inFields := defs.structFields(typeOf[In]())
	for _, f := range inFields {
		if f.codec.invalid != "" {
			return nil, nil, fmt.Errorf("golem: parameter %q: %s", f.name, f.codec.invalid)
		}
	}
	if typeOf[Out]() == typeOf[Unit]() {
		return inFields, nil, nil
	}
	outCodec := defs.compile(typeOf[Out]())
	if outCodec.invalid != "" {
		return nil, nil, fmt.Errorf("golem: result type: %s", outCodec.invalid)
	}
	return inFields, outCodec, nil
}

// typeOf, valueOf and newOf keep the reflect noise out of the client code.
func typeOf[T any]() reflect.Type { return reflect.TypeFor[T]() }

// valueOf addresses through a pointer so an interface-typed value keeps its
// declared type rather than being unwrapped to the concrete one.
func valueOf[T any](p *T) reflect.Value { return reflect.ValueOf(p).Elem() }

func newOf[T any]() reflect.Value { return reflect.New(reflect.TypeFor[T]()).Elem() }
