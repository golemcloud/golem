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

package reflection

import (
	"fmt"
	"time"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
)

// Fully dynamic clients.
//
// These are for callers that already hold schema-native values — an
// infrastructure transport, or a caller that packed them with a discovered
// snapshot — and keep no schema authority. Nothing is validated locally: the
// host still authorises the call and validates the target-side input.
//
//	client, err := reflection.BindAgentID(id)
//	out, invocation, err := client.Call("greet", input)
//
// To combine discovery with a dynamic call, pack with the discovered method's
// Input, call, and read the result with its Output.

// RawAgentID is an agent identity taken apart by the host, with the
// constructor left as a typed value rather than decoded into a Go type — which
// is the point: a dynamic caller has no such type. The typed counterpart is
// [golem.ParseAgentID].
type RawAgentID struct {
	// TypeName is the agent type the identity names.
	TypeName string
	// Constructor carries the caller-supplied constructor fields. The host
	// injects principal fields separately, so a principal-scoped agent can be
	// rebuilt from an identity returned by invocation metadata.
	Constructor golem.TypedValue
	// PhantomID is the phantom instance this identity addresses, if any.
	PhantomID golem.Option[golem.UUID]
}

// ConstructorJSON reads the constructor fields as canonical JSON.
func (p RawAgentID) ConstructorJSON() (any, error) { return p.Constructor.JSON() }

// DynamicAgentClient invokes an existing agent with values the caller packed
// itself.
type DynamicAgentClient struct {
	agentID string
	parsed  RawAgentID
	rpc     agentRPC
}

// AgentID returns the identity this client is bound to.
func (c *DynamicAgentClient) AgentID() string { return c.agentID }

// Parsed returns what the host made of the identity.
func (c *DynamicAgentClient) Parsed() RawAgentID { return c.parsed }

// Call invokes a method with an already-packed parameter record and waits for
// its result, none for a method that returns nothing, together with the
// invocation's identity.
func (c *DynamicAgentClient) Call(method string, input core.SchemaValue) (golem.Option[core.SchemaValue], golem.InvocationID, error) {
	p, err := c.CallAsync(method, input)
	if err != nil {
		return golem.None[core.SchemaValue](), golem.InvocationID{}, err
	}
	out, err := p.Wait()
	return out, p.ID, err
}

// CallAsync invokes a method with an already-packed parameter record and
// returns at once; the result is read, as for Call, with the pending call's
// Wait.
func (c *DynamicAgentClient) CallAsync(method string, input core.SchemaValue) (*PendingCall[golem.Option[core.SchemaValue]], error) {
	tree, err := witschema.ValueToWit(input)
	if err != nil {
		return nil, fmt.Errorf("golem: %s: %w", method, err)
	}
	p, err := c.rpc.start(method, tree)
	if err != nil {
		return nil, err
	}
	return &PendingCall[golem.Option[core.SchemaValue]]{ID: p.id, cancel: p.cancel, wait: func() (golem.Option[core.SchemaValue], error) {
		res, err := p.wait()
		if err != nil {
			return golem.None[core.SchemaValue](), err
		}
		result, has := optionFromWit(res).Get()
		if !has {
			return golem.None[core.SchemaValue](), nil
		}
		value, err := witschema.ValueToCore(result)
		if err != nil {
			return golem.None[core.SchemaValue](), fmt.Errorf("golem: %s returned an unreadable result: %w", method, err)
		}
		return golem.Some(value), nil
	}}, nil
}

// Trigger invokes a method without waiting for its result.
func (c *DynamicAgentClient) Trigger(method string, input core.SchemaValue) (golem.InvocationID, error) {
	tree, err := witschema.ValueToWit(input)
	if err != nil {
		return golem.InvocationID{}, fmt.Errorf("golem: %s: %w", method, err)
	}
	return c.rpc.trigger(method, tree)
}

// Schedule arranges for a method to be invoked at the given time.
func (c *DynamicAgentClient) Schedule(at time.Time, method string, input core.SchemaValue) (*golem.ScheduledInvocation, error) {
	tree, err := witschema.ValueToWit(input)
	if err != nil {
		return nil, fmt.Errorf("golem: %s: %w", method, err)
	}
	return c.rpc.schedule(at, method, tree)
}
